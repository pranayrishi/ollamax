use anyhow::Result;
use clap::Parser;
use ollama_forge::agent::{Agent, AgentConfig};
use ollama_forge::cli::{Cli, Commands, RulesAction, SkillsAction};
use ollama_forge::executor::ProgressEvent;
use ollama_forge::orchestrator::{BuildRequest, Orchestrator, OrchestratorConfig};
use ollama_forge::providers::{GenerateOptions, LlmProvider, OllamaProvider};
use ollama_forge::replay::{quick_hash, read_log};
use ollama_forge::rules::RuleSet;
use ollama_forge::security::{SecurityGuard, Severity};
use ollama_forge::tools::ToolRegistry;
use ollama_forge::{init_tracing, monitoring::VramSentinel, skills::SkillsEngine, Config};
use std::path::{Path, PathBuf};
use tracing::{error, info};

fn main() {
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("forge: failed to start tokio runtime: {e}");
            std::process::exit(2);
        }
    };

    if let Err(e) = runtime.block_on(async_main()) {
        error!("fatal: {e:#}");
        eprintln!("forge: {e:#}");
        std::process::exit(1);
    }
}

async fn async_main() -> Result<()> {
    let cli = Cli::parse();

    // Default to `warn` so the user's normal commands aren't peppered with
    // INFO log lines from the library. `--verbose` brings back the chatty
    // INFO/DEBUG output for debugging.
    let log_level = if cli.verbose {
        "debug"
    } else if cli.quiet {
        "error"
    } else {
        "warn"
    };
    init_tracing(log_level)?;

    let config = match cli.config.as_deref() {
        Some(path) => load_config_from(path)?,
        None => Config::load().await?,
    };

    // Load user "always-rules" from the rules directory and prepare a
    // suffix to append to every system prompt. Empty string when no rules
    // are configured, so call sites can unconditionally concatenate.
    let rules = RuleSet::load_default().unwrap_or_default();
    let rules_suffix = rules.render();

    match cli.command {
        Commands::Init { force } => init_project(force).await?,

        Commands::Build {
            task,
            no_security,
            output,
        } => {
            if task.is_empty() {
                anyhow::bail!("forge build: task is required");
            }
            let request = BuildRequest {
                task: task.join(" "),
                output_dir: None,
                language: None,
                run_tests: false,
                skip_security: no_security,
            };

            let orchestrator = Orchestrator::new(OrchestratorConfig {
                ollama_url: config.ollama_url.clone(),
                default_model: config.default_model.clone(),
                planning_model: config.planning_model.clone(),
                max_parallel_workers: config.max_parallel_workers,
                security_enabled: config.security_enabled && !no_security,
                tdd_enforced: config.tdd_enforced,
                rules_suffix: rules_suffix.clone(),
            })
            .await?;

            // Real per-event progress to stderr. The orchestrator emits a
            // ProgressEvent for each model preload and each worker; we
            // render them as a flat live log so the user knows what's
            // happening during a long parallel build.
            eprintln!("🏗  forge build: orchestrating…");
            let (prog_tx, mut prog_rx) = tokio::sync::mpsc::unbounded_channel::<ProgressEvent>();
            let progress_task = tokio::spawn(async move {
                while let Some(ev) = prog_rx.recv().await {
                    match ev {
                        ProgressEvent::PreloadStarted { model } => {
                            eprintln!("   ⏳ preload  start  {model}");
                        }
                        ProgressEvent::PreloadFinished {
                            model,
                            ok,
                            elapsed_ms,
                        } => {
                            let mark = if ok { "✅" } else { "❌" };
                            eprintln!("   {mark} preload  done   {model}  ({elapsed_ms}ms)");
                        }
                        ProgressEvent::WorkerStarted {
                            subtask_name,
                            model,
                            ..
                        } => {
                            eprintln!("   ⏳ worker   start  {subtask_name:<16} on {model}");
                        }
                        ProgressEvent::WorkerFinished {
                            subtask_name,
                            ok,
                            elapsed_ms,
                            tokens,
                            ..
                        } => {
                            let mark = if ok { "✅" } else { "❌" };
                            eprintln!(
                                "   {mark} worker   done   {subtask_name:<16} {elapsed_ms}ms  {tokens} tok"
                            );
                        }
                    }
                }
            });

            let result = orchestrator
                .execute_with_progress(request, Some(prog_tx))
                .await?;
            // Drop side effects + wait for the renderer to drain.
            let _ = progress_task.await;

            eprintln!(
                "✅ build completed on `{}` ({} tok, {} ms total across workers)",
                result.model_used, result.tokens_generated, result.duration_ms
            );
            if !result.warnings.is_empty() {
                eprintln!("   ⚠️  {} worker(s) failed:", result.warnings.len());
                for w in &result.warnings {
                    eprintln!("      - {w}");
                }
            }

            if let Some(out_dir) = output {
                let written = ollama_forge::codeblocks::extract_and_write_code_blocks(
                    &out_dir,
                    &result.output,
                )?;
                if written.is_empty() {
                    eprintln!(
                        "⚠️  --output was set but the model produced no \
                         labeled code blocks (`\\`\\`\\`lang path/to/file`). \
                         Writing the raw response to {}/build_output.md instead.",
                        out_dir.display()
                    );
                    std::fs::create_dir_all(&out_dir)?;
                    std::fs::write(out_dir.join("build_output.md"), &result.output)?;
                } else {
                    eprintln!(
                        "📦 wrote {} file(s) to {}:",
                        written.len(),
                        out_dir.display()
                    );
                    for w in &written {
                        eprintln!("   - {}", w.display());
                    }
                }
            } else {
                println!("{}", result.output);
            }
        }

        Commands::Chat { model, prompt } => {
            let ollama = OllamaProvider::new(&config.ollama_url);
            // If FORGE_REPLAY_LOG is set we want deterministic output —
            // otherwise the chat is unrepeatable and the log is meaningless.
            // Default to seed=0 + temp=0 in that mode.
            let replay_mode = std::env::var_os("FORGE_REPLAY_LOG").is_some();
            let system = if rules_suffix.is_empty() {
                None
            } else {
                Some(rules_suffix.clone())
            };
            let opts = GenerateOptions {
                model: model.unwrap_or_else(|| config.default_model.clone()),
                prompt: prompt.unwrap_or_default(),
                system,
                stream: true,
                temperature: if replay_mode { Some(0.0) } else { Some(0.7) },
                seed: if replay_mode { Some(0) } else { None },
                ..Default::default()
            };
            // Real streaming: print each token as it arrives. Previously we
            // claimed `stream: true` but `generate()` buffered the whole
            // response, giving the user a 20s wait for a wall of text.
            use std::io::Write;
            let mut stdout = std::io::stdout().lock();
            let mut full = String::new();
            let bytes = ollama
                .generate_streaming(opts.clone(), |chunk| {
                    let _ = stdout.write_all(chunk.as_bytes());
                    let _ = stdout.flush();
                    full.push_str(chunk);
                })
                .await?;
            let _ = writeln!(stdout);
            if bytes == 0 {
                eprintln!("forge: model returned no tokens");
            }
            // Append to replay log if FORGE_REPLAY_LOG is set.
            maybe_log_replay(&opts, &full, &config.ollama_url).await;
        }

        Commands::Status { models } => {
            let ollama = OllamaProvider::new(&config.ollama_url);
            let sentinel = VramSentinel::new(config.min_free_vram_mb, false);
            let health = sentinel.detect_hardware().await;

            println!("\n🖥️  Hardware");
            println!("   OS:      {}", health.os);
            println!("   GPU:     {:?}", health.gpu_kind);
            println!("   RAM:     {} MB total", health.total_ram_mb);
            println!(
                "   VRAM:    {} MB ({} MB free)",
                health.total_vram_mb, health.free_vram_mb
            );
            println!("   CPUs:    {}", health.cpu_cores);

            println!("\n🎯 Recommendation");
            println!("   Model:   {}", health.recommended_model);
            println!("   num_ctx: {}", health.optimal_context);
            println!("   num_gpu: {}", health.optimal_gpu_layers);

            // Always probe Ollama — the most useful single line in this
            // entire CLI is "Ollama is reachable / not reachable". Don't
            // hide it behind --models.
            println!("\n🤖 Ollama  ({})", config.ollama_url);
            let healthy = ollama.health_check().await.unwrap_or(false);
            if !healthy {
                println!("   ❌ unreachable");
                println!("      hint: run `ollama serve` in another terminal");
                return Ok(());
            }
            println!("   ✅ reachable");

            // Show currently-loaded models so the user can tell whether
            // their last `forge preload` actually warmed a model.
            match ollama.running_models().await {
                Ok(running) if running.is_empty() => {
                    println!("   loaded:  (none)");
                }
                Ok(running) => {
                    println!("   loaded:");
                    for m in running {
                        let mb = m.size_vram_bytes / (1024 * 1024);
                        let exp = m
                            .expires_at
                            .as_deref()
                            .map(|e| format!("  expires {e}"))
                            .unwrap_or_default();
                        println!("     - {} ({mb} MB VRAM){exp}", m.name);
                    }
                }
                Err(e) => {
                    println!("   loaded:  (failed to query /api/ps: {e})");
                }
            }

            if models {
                println!("\n📦 Pulled models");
                match ollama.list_models().await {
                    Ok(model_list) if model_list.is_empty() => {
                        println!("   (none — try `ollama pull qwen2.5-coder:7b`)");
                    }
                    Ok(model_list) => {
                        for model in model_list {
                            println!("   - {} ({})", model.name, model.size_human);
                        }
                    }
                    Err(e) => {
                        println!("   (failed to query /api/tags: {e})");
                    }
                }
            } else {
                println!("\n   (run `forge status --models` to see all pulled models)");
            }
        }

        Commands::Models { verify, fits_only } => {
            use ollama_forge::models::{verify_in_library, HardwareTier, ModelRegistry};
            let ollama = OllamaProvider::new(&config.ollama_url);
            let sentinel = VramSentinel::new(config.min_free_vram_mb, false);
            let hw = sentinel.detect_hardware().await;
            let free = hw.free_vram_mb;
            let installed: Vec<String> = ollama
                .list_models()
                .await
                .map(|ms| ms.into_iter().map(|m| m.name).collect())
                .unwrap_or_default();

            let mut reg = ModelRegistry::seed();
            reg.mark_installed(&installed);
            let fits: std::collections::HashSet<String> =
                reg.fits(free).into_iter().map(|m| m.ollama_tag.clone()).collect();
            let recommended = reg.recommend(free, &installed).map(|m| m.ollama_tag.clone());

            println!(
                "\n🖥️  Detected {:?} · {free} MB free VRAM → tier: {}",
                hw.gpu_kind,
                HardwareTier::for_vram(free).label()
            );
            if let Some(r) = &recommended {
                println!("🎯 Recommended for your machine: {r}\n   pull it:  ollama pull {r}");
            }
            println!(
                "\nFree, open-weight models — run locally via Ollama. Cloud models (GPT/Claude/\n\
                 Gemini) are paid, bring-your-own-key, and intentionally not listed here.\n"
            );

            for tier in [HardwareTier::Modest, HardwareTier::Single, HardwareTier::HighEnd] {
                println!("── {} ──", tier.label());
                for m in reg.all().iter().filter(|m| m.tier == tier) {
                    let does_fit = fits.contains(&m.ollama_tag);
                    if fits_only && !does_fit {
                        continue;
                    }
                    let mut flags = Vec::new();
                    if m.installed {
                        flags.push("✓ installed".to_string());
                    }
                    flags.push(if does_fit { "fits".to_string() } else { "needs more VRAM".to_string() });
                    if !m.license.commercial_friendly() {
                        flags.push(format!("⚠ {}", m.license.spdx()));
                    }
                    if verify {
                        match verify_in_library(&m.ollama_tag).await {
                            Some(false) => flags.push("✗ not in library".to_string()),
                            None => flags.push("? unverified".to_string()),
                            Some(true) => {}
                        }
                    }
                    println!(
                        "  {:30} {:11} {:10}  [{}]",
                        m.ollama_tag,
                        m.params,
                        m.license.spdx(),
                        flags.join(", ")
                    );
                }
                println!();
            }
            println!("Pull a tag, then select it in the chat panel or pass `--model <tag>`.");
        }

        Commands::Optimize {
            aggressive: _,
            dry_run,
        } => {
            let sentinel = VramSentinel::new(config.min_free_vram_mb, true);
            let plan = sentinel.auto_optimize().await?;

            println!("\n⚙️  Optimization Plan:");
            println!("   Recommended Model: {}", plan.recommended_model);
            println!("   Optimal Context: {}", plan.optimal_context);
            println!("   GPU Layers: {}", plan.optimal_num_gpu);
            println!("   Keep Alive: {}s", plan.keep_alive_duration);

            if plan.apply_onnx {
                println!("   ⚡ ONNX acceleration recommended for your hardware");
            }

            if !dry_run {
                println!("\nTo apply these settings to Ollama:");
                println!("   ollama create optimized -f - << EOF");
                println!("FROM {}", plan.recommended_model);
                println!("PARAMETER num_ctx {}", plan.optimal_context);
                println!("PARAMETER num_gpu {}", plan.optimal_num_gpu);
                println!("EOF");
            }
        }

        Commands::Replay { log, verbose } => {
            if !log.exists() {
                anyhow::bail!("replay log not found: {}", log.display());
            }
            let records = read_log(&log).await?;
            if records.is_empty() {
                eprintln!("forge replay: log is empty, nothing to do");
                return Ok(());
            }
            eprintln!(
                "🎬 replaying {} record(s) from {}",
                records.len(),
                log.display()
            );
            let ollama = OllamaProvider::new(&config.ollama_url);
            let mut drifted = 0usize;
            let mut matched = 0usize;
            let mut errored = 0usize;

            for (i, rec) in records.iter().enumerate() {
                let opts = GenerateOptions {
                    model: rec.model.clone(),
                    prompt: rec.prompt.clone(),
                    system: rec.system.clone(),
                    temperature: rec.temperature,
                    top_p: rec.top_p,
                    num_ctx: rec.num_ctx,
                    keep_alive: rec.keep_alive.clone(),
                    seed: rec.seed,
                    format: rec.format.clone(),
                    stream: false,
                    ..Default::default()
                };
                eprintln!(
                    "  [{}/{}] {} ({}…) →",
                    i + 1,
                    records.len(),
                    rec.model,
                    &rec.prompt_hash[..rec.prompt_hash.len().min(16)]
                );
                match ollama.generate(opts).await {
                    Ok(resp) => {
                        let new_hash = quick_hash(resp.content.as_bytes());
                        if new_hash == rec.response_hash {
                            matched += 1;
                            eprintln!("       ✅ match");
                        } else {
                            drifted += 1;
                            eprintln!(
                                "       ⚠️  drift  was={}  now={}",
                                rec.response_hash, new_hash
                            );
                            if verbose {
                                eprintln!("       --- recorded response ---");
                                eprintln!("{}", rec.response);
                                eprintln!("       --- new response ---");
                                eprintln!("{}", resp.content);
                            }
                        }
                    }
                    Err(e) => {
                        errored += 1;
                        eprintln!("       ❌ error: {e}");
                    }
                }
            }
            eprintln!();
            eprintln!(
                "replay summary: {} match, {} drift, {} error",
                matched, drifted, errored
            );
            if drifted > 0 || errored > 0 {
                std::process::exit(1);
            }
        }

        Commands::Instincts { log, threshold } => {
            let log_path = match log {
                Some(p) => p,
                None => match std::env::var("FORGE_REPLAY_LOG") {
                    Ok(p) => PathBuf::from(p),
                    Err(_) => {
                        anyhow::bail!(
                            "no replay log specified. Pass a path or set FORGE_REPLAY_LOG."
                        );
                    }
                },
            };
            let report = ollama_forge::instincts::from_log(&log_path, threshold).await?;
            println!("\n🧠 Instincts report from {}", log_path.display());
            println!("   {} total record(s) analyzed", report.total_records);
            if report.total_records == 0 {
                println!("\n   (the log is empty — run forge chat / forge research with FORGE_REPLAY_LOG set)");
                return Ok(());
            }

            if report.repeated_tasks.is_empty() {
                println!("\n📋 Repeated tasks: none yet (need 3+ matches)");
            } else {
                println!(
                    "\n📋 Repeated tasks ({}). Promote any of these to a skill:",
                    report.repeated_tasks.len()
                );
                for p in &report.repeated_tasks {
                    let preview: String = p.canonical.chars().take(70).collect();
                    println!("   ×{}  {preview}", p.count);
                    println!("        models: {:?}", p.models);
                }
                println!();
                println!("   To promote: write a skill JSON for the workflow, then");
                println!("     forge skills add path/to/skill.json");
            }

            if report.repeated_systems.is_empty() {
                println!("\n📋 Repeated system prompts: none yet (need 3+ matches)");
            } else {
                println!(
                    "\n📋 Repeated system prompts ({}). Promote any of these to an always-rule:",
                    report.repeated_systems.len()
                );
                for p in &report.repeated_systems {
                    let preview: String = p.canonical.chars().take(120).collect();
                    println!("   ×{}  {preview}", p.count);
                }
                println!();
                println!("   To promote: forge rules init (if first run), then drop a *.md");
                println!("     into ~/.config/ollama-forge/rules/ with the prompt text.");
            }

            if report.repeated_tool_chains.is_empty() {
                println!("\n📋 Repeated agent tool chains: none yet (need 3+ matches)");
            } else {
                println!(
                    "\n📋 Repeated agent tool chains ({}). Promote any of these to a skill recipe:",
                    report.repeated_tool_chains.len()
                );
                for p in &report.repeated_tool_chains {
                    println!("   ×{}  {}", p.count, p.canonical);
                }
                println!();
                println!("   To promote: write a skill JSON with a recipe whose `steps` mirror");
                println!("     this chain, then `forge skills add path/to/skill.json`.");
            }
        }

        Commands::Rules { action } => {
            let dir = RuleSet::default_dir();
            match action {
                RulesAction::Path => {
                    println!("{}", dir.display());
                }
                RulesAction::Init => {
                    if !dir.exists() {
                        std::fs::create_dir_all(&dir)?;
                    }
                    let starter = dir.join("00-style.md");
                    if starter.exists() {
                        eprintln!("{} already exists; not overwriting.", starter.display());
                    } else {
                        std::fs::write(
                            &starter,
                            "---\nname: 00-style\ndescription: Starter rule. Edit me.\n---\n\
                             Always prefer concise, well-commented code.\n\
                             When you generate Rust, use 4-space indent and avoid `unwrap()` outside of tests.\n",
                        )?;
                        eprintln!("✅ wrote {}", starter.display());
                    }
                    println!(
                        "Edit files under {} to control your always-rules.",
                        dir.display()
                    );
                }
                RulesAction::List => {
                    let set = RuleSet::load_from(dir.clone()).unwrap_or_default();
                    if set.is_empty() {
                        println!(
                            "No rules in {}.\nRun `forge rules init` to create the directory and a starter rule.",
                            dir.display()
                        );
                    } else {
                        println!("\n📋 {} rule(s) in {}:", set.len(), dir.display());
                        for r in &set.rules {
                            let desc = r.description.as_deref().unwrap_or("(no description)");
                            println!("   {}  — {}", r.name, desc);
                            println!("      {}", r.source.display());
                        }
                    }
                }
                RulesAction::Show => {
                    let set = RuleSet::load_from(dir.clone()).unwrap_or_default();
                    if set.is_empty() {
                        eprintln!("No rules configured. Run `forge rules init` first.");
                    } else {
                        print!("{}", set.render());
                    }
                }
                RulesAction::Edit { name } => {
                    // Pick the editor: $VISUAL > $EDITOR > sensible defaults.
                    // We don't fall back to a hardcoded editor without
                    // checking — installing forge shouldn't drag in nano.
                    let editor = std::env::var("VISUAL")
                        .or_else(|_| std::env::var("EDITOR"))
                        .ok();
                    let editor = match editor {
                        Some(e) if !e.trim().is_empty() => e,
                        _ => {
                            anyhow::bail!(
                                "neither $VISUAL nor $EDITOR is set. \
                                 Run `EDITOR=vim forge rules edit` or set it in your shell."
                            );
                        }
                    };
                    if !dir.exists() {
                        std::fs::create_dir_all(&dir)?;
                    }
                    let target = match name {
                        Some(n) => {
                            // Strip a stray `.md` suffix if the user typed
                            // it; we'll add it back. This is the ergonomic
                            // detail people forget.
                            let stem = n.trim_end_matches(".md");
                            let path = dir.join(format!("{stem}.md"));
                            if !path.exists() {
                                // Create an empty stub so the editor opens
                                // a real file, not "empty buffer."
                                std::fs::write(
                                    &path,
                                    format!("---\nname: {stem}\ndescription: \n---\n"),
                                )?;
                                eprintln!("created {}", path.display());
                            }
                            path
                        }
                        None => dir.clone(),
                    };

                    // We use `sh -c` so the user's editor command can be
                    // anything: `vim`, `code -w`, `subl -w`, `emacs`, etc.
                    // Wait for the editor to exit (`status()` blocks).
                    let cmdline = format!("{} \"{}\"", editor, target.display());
                    let status = std::process::Command::new("sh")
                        .arg("-c")
                        .arg(&cmdline)
                        .status();
                    match status {
                        Ok(s) if s.success() => {
                            eprintln!("✅ editor closed cleanly. Re-run any forge command and the rules will reload.");
                        }
                        Ok(s) => {
                            anyhow::bail!("editor exited with status {s}");
                        }
                        Err(e) => {
                            anyhow::bail!("could not launch `{editor}`: {e}");
                        }
                    }
                }
            }
        }

        Commands::Finetune { repo, model } => {
            if !repo.exists() {
                anyhow::bail!("finetune target does not exist: {}", repo.display());
            }
            // Detect repo facts for the planner: language(s), file count.
            // Cheap heuristic — count source files by extension.
            let mut langs: std::collections::BTreeMap<&str, usize> =
                std::collections::BTreeMap::new();
            let mut file_count = 0usize;
            for entry in walkdir::WalkDir::new(&repo)
                .follow_links(false)
                .into_iter()
                .filter_entry(|e| {
                    let n = e.file_name().to_string_lossy().into_owned();
                    !(e.file_type().is_dir()
                        && matches!(
                            n.as_str(),
                            "target" | "node_modules" | ".git" | "dist" | "build" | "venv"
                        ))
                })
                .filter_map(|e| e.ok())
            {
                if !entry.file_type().is_file() {
                    continue;
                }
                let ext = entry
                    .path()
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("");
                let lang = match ext {
                    "rs" => Some("Rust"),
                    "py" => Some("Python"),
                    "ts" | "tsx" => Some("TypeScript"),
                    "js" | "jsx" => Some("JavaScript"),
                    "go" => Some("Go"),
                    "java" => Some("Java"),
                    "rb" => Some("Ruby"),
                    "c" | "h" => Some("C"),
                    "cpp" | "cc" | "hpp" => Some("C++"),
                    _ => None,
                };
                if let Some(l) = lang {
                    *langs.entry(l).or_insert(0) += 1;
                    file_count += 1;
                }
            }
            if file_count == 0 {
                anyhow::bail!(
                    "no source files found under {}. \
                     forge finetune needs at least one .rs/.py/.ts/.js/.go/.java/.rb/.c/.cpp file to train on.",
                    repo.display()
                );
            }
            let primary_lang = langs
                .iter()
                .max_by_key(|(_, c)| **c)
                .map(|(l, _)| *l)
                .unwrap_or("(unknown)");

            // Hardware budget for the planner — small models on small VRAM,
            // big models on big VRAM. The skill itself will recommend a
            // base; we just want to feed it ground truth.
            let sentinel = VramSentinel::new(config.min_free_vram_mb, false);
            let hw = sentinel.detect_hardware().await;
            eprintln!(
                "🎓 forge finetune\n   repo: {}\n   primary lang: {primary_lang}\n   {} source file(s)\n   free VRAM: {} MB ({:?})\n",
                repo.display(),
                file_count,
                hw.free_vram_mb,
                hw.gpu_kind
            );

            // Load the lora-finetune skill and run it directly. We could
            // also have the user invoke `forge run-skill lora-finetune` but
            // that requires them to know the skill name and trigger words.
            // The whole point of `forge finetune` is to bootstrap the
            // workflow without that ceremony.
            let skills_dir = dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("ollama-forge")
                .join("skills");
            let engine = SkillsEngine::new(skills_dir);
            engine.load_skills().await?;
            let skill = engine
                .find_skill("lora-finetune")
                .await
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "lora-finetune skill not found. \
                         The bundled recipe should have been written to your skills dir on first run."
                    )
                })?;

            let ollama = OllamaProvider::new(&config.ollama_url);
            let chosen_model = match model {
                Some(m) => m,
                None => pick_installed_model(&config, &ollama).await?,
            };
            let mut system_prompt = skill.prompts.system.clone();
            if let Some(planning) = &skill.prompts.planning {
                system_prompt.push_str("\n\nPlanning guidance: ");
                system_prompt.push_str(planning);
            }
            if let Some(execution) = &skill.prompts.execution {
                system_prompt.push_str("\n\nExecution guidance: ");
                system_prompt.push_str(execution);
            }
            system_prompt.push_str(&rules_suffix);

            let task = format!(
                "Fine-tune a local model on the repo at `{}`. \
                 Primary language: {primary_lang}. {} source files. \
                 Free VRAM: {} MB on a {:?}. \
                 Output the dataset-prep script, the Unsloth training script, \
                 the GGUF conversion script, and the Ollama Modelfile as labeled \
                 fenced code blocks (```python prepare_dataset.py, etc.) so the \
                 user can pipe this output to `forge build --output ./finetune/`.",
                repo.display(),
                file_count,
                hw.free_vram_mb,
                hw.gpu_kind
            );

            let opts = GenerateOptions {
                model: chosen_model.clone(),
                prompt: task,
                system: Some(system_prompt),
                temperature: Some(0.3),
                num_ctx: Some(hw.optimal_context),
                stream: true,
                keep_alive: Some("1h".to_string()),
                ..Default::default()
            };

            eprintln!("running on `{chosen_model}`...\n");
            use std::io::Write;
            let mut stdout = std::io::stdout().lock();
            let bytes = ollama
                .generate_streaming(opts, |chunk| {
                    let _ = stdout.write_all(chunk.as_bytes());
                    let _ = stdout.flush();
                })
                .await?;
            let _ = writeln!(stdout);
            if bytes == 0 {
                eprintln!("forge: model returned no tokens");
            }
            eprintln!();
            eprintln!("Pipe this output through `forge build --output ./finetune/` to extract");
            eprintln!("the scripts to disk:");
            eprintln!("  forge finetune {} > /tmp/lora.md && forge build \"$(cat /tmp/lora.md)\" -o ./finetune/", repo.display());
        }

        Commands::Tools => {
            let registry = ToolRegistry::with_defaults();
            println!(
                "\n🛠  Tools available to the research agent ({} total):",
                registry.len()
            );
            println!();
            // We can't iterate through registry directly without leaking
            // its internal map; print via the description function which
            // is what the model itself sees.
            print!("{}", registry.describe_for_model());
            println!();
            println!("All tool endpoints are free, no API key required:");
            println!("  - web_search → DuckDuckGo Instant Answer JSON API");
            println!("  - wikipedia  → Wikipedia REST + opensearch");
            println!("  - arxiv      → arXiv Atom API");
            println!("  - fetch_url  → plain HTTP GET (no service in front)");
        }

        Commands::Research {
            question,
            model,
            trace,
            max_iterations,
        } => {
            if question.is_empty() {
                anyhow::bail!("forge research: question is required");
            }
            let question = question.join(" ");

            let ollama = std::sync::Arc::new(OllamaProvider::new(&config.ollama_url));
            let chosen_model = match model {
                Some(m) => m,
                None => pick_installed_model(&config, &ollama).await?,
            };

            let sentinel = VramSentinel::new(config.min_free_vram_mb, false);
            let hw = sentinel.detect_hardware().await;

            let registry = ToolRegistry::with_defaults();
            eprintln!(
                "🔬 research: `{question}`\n   model: `{chosen_model}`  num_ctx: {}  tools: {}",
                hw.optimal_context,
                registry.len()
            );
            // Honest egress note: inference is local, but the web tools send
            // queries + fetched page text off the machine (mirrors the server
            // disclosure for the chat tools toggle).
            eprintln!(
                "   🌐 inference stays local, but search queries + fetched pages leave your \
                 machine (DuckDuckGo / Wikipedia / arXiv / fetched URLs)."
            );
            eprintln!();

            let mut agent = Agent::new(
                ollama,
                registry,
                AgentConfig {
                    model: chosen_model,
                    num_ctx: hw.optimal_context,
                    keep_alive: "1h".to_string(),
                    max_iterations,
                    system_suffix: rules_suffix.clone(),
                },
            );

            // Trace preview width: 300 chars by default. URLs and citations
            // routinely overflow 100, which made the live trace useless on
            // real research questions. Override via FORGE_TRACE_WIDTH=N.
            let preview_width: usize = std::env::var("FORGE_TRACE_WIDTH")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(300);
            let agent_trace = agent
                .run(&question, |step| {
                    let preview: String = step
                        .result_preview
                        .replace('\n', " ")
                        .chars()
                        .take(preview_width)
                        .collect();
                    eprintln!(
                        "   [round {}] {} ({}) → {}",
                        step.iteration,
                        step.tool,
                        if step.ok { "ok" } else { "FAIL" },
                        preview
                    );
                })
                .await?;

            eprintln!();
            if agent_trace.iteration_capped {
                eprintln!("⚠️  agent hit iteration cap before producing an answer.");
            }
            println!("{}", agent_trace.answer);

            if trace {
                eprintln!();
                eprintln!("📋 trace ({} steps):", agent_trace.steps.len());
                for s in &agent_trace.steps {
                    eprintln!(
                        "   round {} | {} | args={} | ok={}",
                        s.iteration, s.tool, s.args, s.ok
                    );
                }
            }
        }

        Commands::RunSkill { name, task } => {
            if task.is_empty() {
                anyhow::bail!("forge run-skill: task description is required");
            }
            let task = task.join(" ");

            let skills_dir = dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("ollama-forge")
                .join("skills");
            let engine = SkillsEngine::new(skills_dir);
            engine.load_skills().await?;

            let skill = engine.find_skill(&name).await.ok_or_else(|| {
                anyhow::anyhow!("no skill named `{name}` — try `forge skills list`")
            })?;

            // Pick model: skill's recommended → if unavailable locally,
            // fall back through installed models in size order. This avoids
            // the "skill recommends qwen2.5-coder:7b but you only have
            // llama3.1:8b installed → 404" trap.
            let sentinel = VramSentinel::new(config.min_free_vram_mb, false);
            let hw = sentinel.detect_hardware().await;
            let num_ctx = hw.optimal_context;
            let ollama_for_pick = OllamaProvider::new(&config.ollama_url);
            let installed = ollama_for_pick.list_models().await.unwrap_or_default();
            let preferred = skill
                .settings
                .model
                .clone()
                .unwrap_or_else(|| config.default_model.clone());
            let model = if installed.iter().any(|m| m.name == preferred) {
                preferred
            } else if let Some(fallback) = installed.iter().max_by_key(|m| m.size) {
                eprintln!(
                    "⚠️  skill recommends `{preferred}` but it's not installed; \
                     falling back to `{}`",
                    fallback.name
                );
                fallback.name.clone()
            } else {
                anyhow::bail!(
                    "no models installed in ollama. Pull one first:\n  ollama pull {preferred}"
                );
            };
            let temperature = skill.settings.temperature.unwrap_or(0.5);

            // Build the prompt: skill system prompt + planning hint + the task,
            // then append any user always-rules so personal style/conventions
            // are honored even inside skill invocations.
            let mut system_prompt = skill.prompts.system.clone();
            if let Some(planning) = &skill.prompts.planning {
                system_prompt.push_str("\n\nPlanning guidance: ");
                system_prompt.push_str(planning);
            }
            if let Some(execution) = &skill.prompts.execution {
                system_prompt.push_str("\n\nExecution guidance: ");
                system_prompt.push_str(execution);
            }
            system_prompt.push_str(&rules_suffix);

            eprintln!(
                "🛠️  running skill `{}` on `{model}` (num_ctx={num_ctx}, temp={temperature})",
                skill.name
            );
            eprintln!();

            let ollama = OllamaProvider::new(&config.ollama_url);
            let opts = GenerateOptions {
                model: model.clone(),
                prompt: task,
                system: Some(system_prompt),
                temperature: Some(temperature),
                num_ctx: Some(num_ctx),
                stream: true,
                keep_alive: Some("1h".to_string()),
                ..Default::default()
            };

            // Stream so the user sees output as it arrives.
            use std::io::Write;
            let mut stdout = std::io::stdout().lock();
            let bytes = ollama
                .generate_streaming(opts, |chunk| {
                    let _ = stdout.write_all(chunk.as_bytes());
                    let _ = stdout.flush();
                })
                .await?;
            let _ = writeln!(stdout);
            if bytes == 0 {
                eprintln!("forge: model returned no tokens");
            }
        }

        Commands::Skills { action } => {
            let skills_dir = dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("ollama-forge")
                .join("skills");

            let engine = SkillsEngine::new(skills_dir);
            engine.load_skills().await?;

            match action {
                SkillsAction::List => {
                    let skills = engine.list_skills().await;
                    println!("\n📚 Available Skills:");
                    for skill in skills {
                        println!("   {} - {}", skill.name, skill.description);
                    }
                }
                SkillsAction::Add { source } => {
                    // Source must be a local path. Remote URL fetching is
                    // intentionally not supported in v0.1.0 — pulling
                    // arbitrary skill JSON over HTTP would punch a hole
                    // through the "no network calls but ollama" property.
                    let path = std::path::PathBuf::from(&source);
                    if !path.exists() {
                        anyhow::bail!(
                            "skill file not found: {source}\n\
                             (remote URLs are not supported in v0.1.0; \
                             download the file first)"
                        );
                    }
                    let raw = tokio::fs::read_to_string(&path).await?;
                    let skill: ollama_forge::skills::Skill = serde_json::from_str(&raw)
                        .map_err(|e| anyhow::anyhow!("invalid skill JSON in {source}: {e}"))?;
                    let name = skill.name.clone();
                    engine.add_skill(skill).await?;
                    println!("✅ added skill `{name}` from {source}");
                }
                SkillsAction::Remove { name } => {
                    engine.remove_skill(&name).await?;
                    println!("Removed skill: {name}");
                }
                SkillsAction::Search { query } => {
                    let matches = engine.find_all_matching(&query).await;
                    if matches.is_empty() {
                        println!("No skills match: {query}");
                    } else {
                        println!("\n🔍 {} skill(s) match `{query}`:", matches.len());
                        for skill in matches {
                            println!("   {} — {}", skill.name, skill.description);
                        }
                    }
                }
            }
        }

        Commands::Preload { model, keep_alive } => {
            let ollama = OllamaProvider::new(&config.ollama_url);
            let model = model.unwrap_or_else(|| config.default_model.clone());

            // Spawn a spinner so a 14B cold-load doesn't look like a hang.
            // The spinner runs on a background tokio task and is cancelled
            // via a oneshot channel as soon as preload returns.
            let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
            let label = format!("warming `{model}` (keep_alive={keep_alive})");
            let spinner = tokio::spawn(spinner_task(label, cancel_rx));

            let start = std::time::Instant::now();
            let result = ollama.preload(&model, &keep_alive).await;
            let _ = cancel_tx.send(());
            let _ = spinner.await;

            use std::io::Write;
            let mut stderr = std::io::stderr().lock();
            match result {
                Ok(()) => {
                    let _ = writeln!(
                        stderr,
                        "✅ warmed `{model}` in {:.1}s (keep_alive={keep_alive})",
                        start.elapsed().as_secs_f64()
                    );
                    let _ = writeln!(stderr);
                    let _ = writeln!(stderr, "Next call to this model will skip cold-start.");
                    let _ = writeln!(stderr, "Verify with: forge status");
                }
                Err(e) => {
                    let _ = writeln!(stderr, "❌ preload failed");
                    let _ = writeln!(stderr, "forge: {e:#}");
                    let _ = writeln!(
                        stderr,
                        "       is `ollama serve` running at {}?",
                        config.ollama_url
                    );
                    let _ = writeln!(
                        stderr,
                        "       has `{model}` been pulled? try `ollama pull {model}`"
                    );
                    std::process::exit(1);
                }
            }
        }

        Commands::Audit {
            path,
            secrets: _,
            json,
        } => {
            if !path.exists() {
                anyhow::bail!("audit path does not exist: {}", path.display());
            }
            let guard = SecurityGuard::new(true);
            let report = guard.audit_directory(&path).await?;

            if json {
                // Stable JSON shape: the user can pipe this into jq.
                // Severity is lower-cased so it's grep-friendly.
                // `schema_version` lets downstream consumers fail loud if we
                // ever change the shape — bump it whenever a field is added,
                // removed, or renamed.
                let payload = serde_json::json!({
                    "schema_version": 1,
                    "forge_version": ollama_forge::cli::VERSION,
                    "path": path.display().to_string(),
                    "files_scanned": report.files_scanned,
                    "summary": report.summary,
                    "findings": report.findings.iter().map(|f| serde_json::json!({
                        "severity": format!("{:?}", f.rule.severity).to_lowercase(),
                        "rule": f.rule.name,
                        "description": f.rule.description,
                        "file": f.file,
                        "line": f.line_number,
                        "snippet": f.line_content,
                    })).collect::<Vec<_>>(),
                });
                println!("{}", serde_json::to_string_pretty(&payload)?);
                let crit = report
                    .findings
                    .iter()
                    .filter(|f| f.rule.severity == Severity::Critical)
                    .count();
                let high = report
                    .findings
                    .iter()
                    .filter(|f| f.rule.severity == Severity::High)
                    .count();
                if crit > 0 || high > 0 {
                    std::process::exit(1);
                }
                return Ok(());
            }

            println!("\n🔒 Security audit: {}", path.display());
            println!("   {}", report.summary);

            if report.findings.is_empty() {
                println!("\n✅ No findings.");
                return Ok(());
            }

            // Group + count by severity for the header line.
            let crit = report
                .findings
                .iter()
                .filter(|f| f.rule.severity == Severity::Critical)
                .count();
            let high = report
                .findings
                .iter()
                .filter(|f| f.rule.severity == Severity::High)
                .count();
            let med = report
                .findings
                .iter()
                .filter(|f| f.rule.severity == Severity::Medium)
                .count();
            let low = report
                .findings
                .iter()
                .filter(|f| f.rule.severity == Severity::Low)
                .count();
            println!("   critical={crit}  high={high}  medium={med}  low={low}");
            println!();

            for f in &report.findings {
                let badge = match f.rule.severity {
                    Severity::Critical => "CRIT",
                    Severity::High => "HIGH",
                    Severity::Medium => "MED ",
                    Severity::Low => "LOW ",
                    Severity::Info => "INFO",
                };
                let file = f.file.as_deref().unwrap_or("<unknown>");
                println!("  [{badge}] {}:{}  {}", file, f.line_number, f.rule.name);
                println!("         {}", f.rule.description);
            }

            // Non-zero exit when anything Critical/High was found, so this can
            // be used in CI / pre-commit hooks: `forge audit src/ || exit 1`.
            if crit > 0 || high > 0 {
                std::process::exit(1);
            }
        }

        Commands::Analyze {
            path,
            analysis_type,
        } => {
            run_analyze(&config, path, analysis_type, &rules_suffix).await?;
        }

        Commands::Test { path, framework } => {
            run_test_gen(&config, path, framework, &rules_suffix).await?;
        }

        Commands::Parallel { .. } => {
            anyhow::bail!(
                "`forge parallel` is not implemented in v0.1.0. \
                 Use `forge build` for parallel orchestration."
            );
        }

        Commands::Serve { port, host } => {
            ollama_forge::server::run(config, host, port).await?;
        }
    }

    Ok(())
}

/// Render a braille spinner to stderr until `cancel` fires. Used by
/// long-running operations (model preload, future build) so the user knows
/// the process is alive. Renders to stderr so it doesn't pollute piped
/// stdout output. Erases itself on exit.
async fn spinner_task(label: String, mut cancel: tokio::sync::oneshot::Receiver<()>) {
    use std::io::Write;
    // Don't render if stderr isn't a TTY — would just spam control chars
    // into a log file. (Quick approximation: check the env, no isatty crate.)
    if std::env::var_os("FORGE_NO_SPINNER").is_some() {
        return;
    }
    const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let mut tick = 0usize;
    let start = std::time::Instant::now();
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(80));
    loop {
        tokio::select! {
            biased;
            _ = &mut cancel => break,
            _ = interval.tick() => {
                let elapsed = start.elapsed().as_secs_f64();
                let frame = FRAMES[tick % FRAMES.len()];
                tick = tick.wrapping_add(1);
                let mut stderr = std::io::stderr().lock();
                let _ = write!(stderr, "\r{frame} {label}  {elapsed:>5.1}s");
                let _ = stderr.flush();
            }
        }
    }
    // Erase the spinner line so the success/failure message starts clean.
    let mut stderr = std::io::stderr().lock();
    let _ = write!(stderr, "\r\x1b[2K");
    let _ = stderr.flush();
}

/// If `FORGE_REPLAY_LOG` is set, append a record for this Ollama call to it.
/// Best-effort: failures here only log a warning, never break the user's
/// command. The log is opt-in because not every user wants their prompts
/// persisted to disk.
async fn maybe_log_replay(opts: &GenerateOptions, response: &str, ollama_url: &str) {
    let Ok(path) = std::env::var("FORGE_REPLAY_LOG") else {
        return;
    };
    // Look up the model digest via /api/show. Best-effort: empty string in
    // the log just means we couldn't reach Ollama mid-write. Without the
    // digest, replay can't tell if the user pulled a different version of
    // the same tag — so this lookup is critical to the audit-trail pitch.
    let provider_for_digest = OllamaProvider::new(ollama_url);
    let digest = provider_for_digest
        .model_digest(&opts.model)
        .await
        .unwrap_or_default();
    let log = ollama_forge::replay::ReplayLog::new(std::path::PathBuf::from(path));
    // Hash everything that determines a deterministic response so the
    // replay can detect drift on weights/sampler/format changes.
    let mut prompt_material = String::new();
    if let Some(s) = &opts.system {
        prompt_material.push_str(s);
        prompt_material.push('\n');
    }
    prompt_material.push_str(&opts.prompt);
    if let Some(f) = &opts.format {
        prompt_material.push('\n');
        prompt_material.push_str(&f.to_string());
    }
    let record = ollama_forge::replay::ReplayRecord {
        ts: chrono::Utc::now().to_rfc3339(),
        forge_version: ollama_forge::cli::VERSION.to_string(),
        model: opts.model.clone(),
        model_digest: digest,
        temperature: opts.temperature,
        top_p: opts.top_p,
        num_ctx: opts.num_ctx,
        keep_alive: opts.keep_alive.clone(),
        seed: opts.seed,
        format: opts.format.clone(),
        system: opts.system.clone(),
        prompt: opts.prompt.clone(),
        prompt_hash: ollama_forge::replay::quick_hash(prompt_material.as_bytes()),
        response_hash: ollama_forge::replay::quick_hash(response.as_bytes()),
        response: response.chars().take(16_384).collect(),
    };
    if let Err(e) = log.append(&record).await {
        tracing::warn!("forge replay log append failed: {e}");
    }
}

/// Starter `forge.toml` written by `forge init`. This is a const string —
/// previously it was `include_str!("../forge.toml")` which baked the
/// developer's *own* `forge.toml` (including any local edits) into every
/// release binary.
const STARTER_FORGE_TOML: &str = r#"[forge]
version = "1.0"

[ollama]
url = "http://localhost:11434"
default_model = "qwen2.5-coder:7b"
planning_model = "qwen2.5-coder:7b"

[execution]
parallel_workers = 4
max_context_tokens = 16384

[security]
enabled = true
scan_secrets = true

[tdd]
enforced = false
"#;

async fn init_project(force: bool) -> Result<()> {
    let forge_toml = PathBuf::from("forge.toml");

    if forge_toml.exists() && !force {
        println!(
            "forge: forge.toml already exists in {}.",
            std::env::current_dir()?.display()
        );
        println!("       re-run with --force to overwrite.");
        return Ok(());
    }

    tokio::fs::write(&forge_toml, STARTER_FORGE_TOML).await?;
    info!(
        "initialized forge project at {}",
        std::env::current_dir()?.display()
    );
    println!("✅ forge.toml written.");
    println!();
    println!("Next:");
    println!("   forge status                 # detect hardware, recommend a model");
    println!("   forge preload                # warm-load the recommended model");
    println!("   forge audit src/             # scan your code for leaked secrets");
    println!("   forge chat \"hello\"           # try the model interactively");
    Ok(())
}

fn load_config_from(path: &Path) -> Result<Config> {
    let content = std::fs::read_to_string(path)?;
    Ok(serde_yaml::from_str(&content)?)
}

/// Pick the most-capable installed Ollama model. Prefers `config.default_model`
/// if it's installed; otherwise falls back to the largest installed model;
/// otherwise errors.
async fn pick_installed_model(config: &Config, ollama: &OllamaProvider) -> Result<String> {
    let installed = ollama
        .list_models()
        .await
        .map_err(|e| anyhow::anyhow!("could not list ollama models: {e}"))?;
    if installed.iter().any(|m| m.name == config.default_model) {
        return Ok(config.default_model.clone());
    }
    if let Some(biggest) = installed.iter().max_by_key(|m| m.size) {
        eprintln!(
            "⚠️  configured `{}` is not installed; using `{}`",
            config.default_model, biggest.name
        );
        return Ok(biggest.name.clone());
    }
    anyhow::bail!(
        "no models installed in ollama. Pull one first:\n  ollama pull {}",
        config.default_model
    );
}

/// `forge analyze <path>` — runs the local secret scanner *and* asks a model
/// to review the code for bugs/quality. Two passes that complement each other:
/// the regex pass finds the things you can pin with a pattern (secrets,
/// dangerous commands), the model pass finds the things you can't (logic
/// bugs, missing error handling, performance traps).
async fn run_analyze(
    config: &Config,
    path: PathBuf,
    analysis_type: Option<ollama_forge::cli::AnalysisType>,
    rules_suffix: &str,
) -> Result<()> {
    use ollama_forge::cli::AnalysisType;

    if !path.exists() {
        anyhow::bail!("analyze path does not exist: {}", path.display());
    }

    let kind = analysis_type.unwrap_or(AnalysisType::Full);
    let do_security = matches!(kind, AnalysisType::Security | AnalysisType::Full);
    let do_review = matches!(
        kind,
        AnalysisType::Complexity
            | AnalysisType::Performance
            | AnalysisType::Style
            | AnalysisType::Full
    );

    if do_security {
        let guard = SecurityGuard::new(true);
        let report = guard.audit_directory(&path).await?;
        println!("\n🔒 Security pass: {}", report.summary);
        for f in &report.findings {
            println!(
                "  [{:?}] {}:{}  {}",
                f.rule.severity,
                f.file.as_deref().unwrap_or("?"),
                f.line_number,
                f.rule.name
            );
        }
    }

    if do_review {
        // Token-aware budgeting: read files until we approach the model's
        // optimal context, leaving headroom for the prompt template + the
        // model's response. The previous version capped at 50 KB which
        // overshot 16k context budgets on dense code (50 KB ≈ 12-15k tokens
        // *just for the input*).
        let sentinel_for_budget = VramSentinel::new(config.min_free_vram_mb, false);
        let hw_for_budget = sentinel_for_budget.detect_hardware().await;
        // Reserve ~30% of context for the response and prompt scaffolding.
        let max_input_tokens = (hw_for_budget.optimal_context * 7) / 10;
        let mut combined = String::new();
        let mut combined_tokens = 0usize;
        let mut files_seen = 0usize;
        for entry in walkdir::WalkDir::new(&path)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                let n = e.file_name().to_string_lossy().into_owned();
                !(e.file_type().is_dir()
                    && matches!(
                        n.as_str(),
                        "target" | "node_modules" | ".git" | "dist" | "build"
                    ))
            })
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let ext = entry
                .path()
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            if !matches!(
                ext,
                "rs" | "py" | "ts" | "tsx" | "js" | "go" | "java" | "rb" | "c" | "cpp" | "h"
            ) {
                continue;
            }
            if let Ok(content) = std::fs::read_to_string(entry.path()) {
                let header = format!("\n// === {} ===\n", entry.path().display());
                let chunk_tokens = ollama_forge::context::estimate_tokens(&header)
                    + ollama_forge::context::estimate_tokens(&content);
                if combined_tokens + chunk_tokens > max_input_tokens {
                    combined.push_str("\n// ... (truncated to fit context budget)\n");
                    break;
                }
                combined.push_str(&header);
                combined.push_str(&content);
                combined_tokens += chunk_tokens;
                files_seen += 1;
            }
        }

        if files_seen == 0 {
            println!("\n📋 Code review pass: no scannable files found.");
            return Ok(());
        }

        let ollama = OllamaProvider::new(&config.ollama_url);
        let model = pick_installed_model(config, &ollama).await?;
        let sentinel = VramSentinel::new(config.min_free_vram_mb, false);
        let hw = sentinel.detect_hardware().await;

        let kind_label = format!("{kind:?}").to_lowercase();
        println!(
            "\n📋 Code review pass ({kind_label}): {files_seen} file(s), \
             ~{combined_tokens} tokens, reviewing on `{model}`"
        );
        println!();

        let mut review_system = "You are a senior code reviewer. Output a numbered list of \
             concrete issues. If no issues exist, say so explicitly."
            .to_string();
        review_system.push_str(rules_suffix);
        let opts = GenerateOptions {
            model,
            prompt: format!(
                "Review the following code. Focus on: {kind_label}.\n\
                 List the top 5 issues with file:line references.\n\
                 Be concrete; do not invent issues that aren't there.\n\n\
                 {combined}"
            ),
            system: Some(review_system),
            temperature: Some(0.2),
            num_ctx: Some(hw.optimal_context),
            stream: true,
            keep_alive: Some("1h".to_string()),
            ..Default::default()
        };

        use std::io::Write;
        let mut stdout = std::io::stdout().lock();
        ollama
            .generate_streaming(opts, |chunk| {
                let _ = stdout.write_all(chunk.as_bytes());
                let _ = stdout.flush();
            })
            .await?;
        let _ = writeln!(stdout);
    }

    Ok(())
}

/// `forge test <path> [--framework=...]` — generates a test file for the
/// target source file using the model. Streams output to stdout so the user
/// can pipe it directly into a file.
async fn run_test_gen(
    config: &Config,
    path: PathBuf,
    framework: Option<String>,
    rules_suffix: &str,
) -> Result<()> {
    if !path.exists() || !path.is_file() {
        anyhow::bail!(
            "test path must be an existing file (got: {})",
            path.display()
        );
    }
    let source = std::fs::read_to_string(&path)?;
    if source.trim().is_empty() {
        anyhow::bail!("source file is empty: {}", path.display());
    }

    // Detect language from extension; that drives the default framework choice.
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_string();
    let (language, default_framework) = match ext.as_str() {
        "rs" => ("Rust", "the standard #[test] / #[tokio::test] attribute"),
        "py" => ("Python", "pytest"),
        "ts" | "tsx" => ("TypeScript", "Vitest"),
        "js" | "jsx" => ("JavaScript", "Vitest"),
        "go" => ("Go", "the standard testing package"),
        "java" => ("Java", "JUnit 5"),
        "rb" => ("Ruby", "RSpec"),
        _ => ("the source language", "the standard test framework"),
    };
    let framework = framework.unwrap_or_else(|| default_framework.to_string());

    let ollama = OllamaProvider::new(&config.ollama_url);
    let model = pick_installed_model(config, &ollama).await?;
    let sentinel = VramSentinel::new(config.min_free_vram_mb, false);
    let hw = sentinel.detect_hardware().await;

    eprintln!(
        "🧪 generating {language} tests for {} on `{model}` (framework: {framework})",
        path.display()
    );
    eprintln!();

    let mut test_system = "You are a senior test engineer. Write production-quality tests \
         that compile and run on the first try."
        .to_string();
    test_system.push_str(rules_suffix);
    let opts = GenerateOptions {
        model,
        prompt: format!(
            "Write a complete test file for the following {language} source code. \
             Use {framework}. Cover happy paths, error cases, and edge cases. \
             Output ONLY the test file contents — no markdown fences, no \
             explanation, no preamble.\n\n\
             === source: {} ===\n{source}\n",
            path.display()
        ),
        system: Some(test_system),
        temperature: Some(0.3),
        num_ctx: Some(hw.optimal_context),
        stream: true,
        keep_alive: Some("1h".to_string()),
        ..Default::default()
    };

    use std::io::Write;
    let mut stdout = std::io::stdout().lock();
    ollama
        .generate_streaming(opts, |chunk| {
            let _ = stdout.write_all(chunk.as_bytes());
            let _ = stdout.flush();
        })
        .await?;
    let _ = writeln!(stdout);

    Ok(())
}
