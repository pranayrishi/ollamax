use anyhow::Result;
use clap::Parser;
use ollama_forge::agent::{Agent, AgentConfig, Approval, ApprovalPolicy};
use ollama_forge::cli::{
    AgentAutonomy, Cli, Commands, EvalAction, PluginsAction, RulesAction, SkillsAction,
};
use ollama_forge::evals::{
    compare_records, load_scenario, score_records, JsonlEvaluationStore, ScoreComparison,
    ScoreReport,
};
use ollama_forge::executor::ProgressEvent;
use ollama_forge::models::is_offline_ollama_tag;
use ollama_forge::orchestrator::{BuildRequest, Orchestrator, OrchestratorConfig};
use ollama_forge::plugins::{render_context_suffix, PluginManager, MAX_RELEVANT_PLUGINS};
use ollama_forge::providers::{
    parse_local_model_selector, ConfiguredLocalModelMetadata, GenerateOptions, LlmProvider,
    LocalEndpointRegistry, OllamaProvider,
};
use ollama_forge::replay::{quick_hash, read_log};
use ollama_forge::rules::RuleSet;
use ollama_forge::security::{SecurityGuard, Severity};
use ollama_forge::team::{
    TeamConfig, TeamCoordinator, TeamEvent, TeamMode, TeamProviders, TeamStatus,
};
use ollama_forge::tools::{
    files::{FsEditTool, FsListTool, FsReadTool, FsSearchTool, FsWriteTool, WorkspaceFs},
    shell::{ShellPolicy, ShellTool},
    ToolRegistry,
};
use ollama_forge::{init_tracing, monitoring::VramSentinel, skills::SkillsEngine, Config};
use std::path::PathBuf;
use std::sync::Arc;
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

    // `init --force` is the recovery path for a broken project config, so do
    // not parse that config before it has a chance to overwrite it.
    let config = if matches!(&cli.command, Commands::Init { .. }) {
        Config::default()
    } else {
        match cli.config.as_deref() {
            Some(path) => Config::load_from_path(path)?,
            None => Config::load().await?,
        }
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
            let ollama = Arc::new(OllamaProvider::new(&config.ollama_url));
            let local_endpoints = LocalEndpointRegistry::from_config(&config)?;
            let target = select_cli_model(&config, &local_endpoints, &ollama, model).await?;
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
                model: target.model.clone(),
                prompt: prompt.unwrap_or_default(),
                system,
                stream: true,
                temperature: if replay_mode { Some(0.0) } else { Some(0.7) },
                seed: if replay_mode { Some(0) } else { None },
                ..Default::default()
            };
            if let Some(local) = &target.local {
                print_configured_local_target("chat", local);
                eprintln!(
                    "   note: configured OpenAI-compatible endpoints use a bounded buffered completion in the CLI."
                );
                let buffered_opts = GenerateOptions {
                    stream: false,
                    ..opts.clone()
                };
                let response = target
                    .provider
                    .generate(buffered_opts.clone())
                    .await
                    .map_err(|error| {
                        anyhow::anyhow!("forge chat via `{}`: {error:#}", target.model)
                    })?;
                println!("{}", response.content);
                if response.content.trim().is_empty() {
                    eprintln!("forge: model returned no tokens");
                }
                maybe_log_replay(&buffered_opts, &response.content, target.provider.as_ref()).await;
            } else {
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
                maybe_log_replay(&opts, &full, ollama.as_ref()).await;
            }
        }

        Commands::Agent {
            model,
            max_iterations,
            autonomy,
            yes,
            task,
        } => {
            if task.is_empty() {
                anyhow::bail!("forge agent: task is required");
            }
            run_workspace_agent(
                &config,
                &rules_suffix,
                task.join(" "),
                model,
                max_iterations,
                if yes { AgentAutonomy::Auto } else { autonomy },
            )
            .await?;
        }

        Commands::Team {
            model,
            scout_model,
            planner_model,
            reviewer_model,
            max_iterations,
            max_repair_rounds,
            parallel_scouts,
            autonomy,
            yes,
            task,
        } => {
            if task.is_empty() {
                anyhow::bail!("forge team: task is required");
            }
            let team_mode = if parallel_scouts
                && config.enable_parallel
                && config.max_parallel_workers >= 2
            {
                TeamMode::ParallelScouts
            } else {
                if parallel_scouts {
                    eprintln!(
                        "⚠️  --parallel-scouts was requested, but this configuration does not allow two workers; using serial scouts."
                    );
                }
                TeamMode::Serial
            };
            run_workspace_team(
                &config,
                &rules_suffix,
                WorkspaceTeamRequest {
                    task: task.join(" "),
                    requested_model: model,
                    requested_scout_model: scout_model,
                    requested_planner_model: planner_model,
                    reviewer_model,
                    max_iterations,
                    max_repair_rounds,
                    mode: team_mode,
                    autonomy: if yes { AgentAutonomy::Auto } else { autonomy },
                },
            )
            .await?;
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
                        println!("   (none — try `ollama pull qwen3.5:4b`)");
                    }
                    Ok(model_list) => {
                        for model in model_list {
                            if !is_offline_ollama_tag(&model.name) {
                                continue;
                            }
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
                .map(|ms| {
                    ms.into_iter()
                        .filter(|model| is_offline_ollama_tag(&model.name))
                        .map(|model| model.name)
                        .collect()
                })
                .unwrap_or_default();

            let mut reg = ModelRegistry::seed();
            reg.mark_installed(&installed);
            let fits: std::collections::HashSet<String> = reg
                .fits(free)
                .into_iter()
                .map(|m| m.ollama_tag.clone())
                .collect();
            let recommended = reg
                .recommend(free, &installed)
                .map(|m| m.ollama_tag.clone());

            println!(
                "\n🖥️  Detected {:?} · {free} MB free VRAM → tier: {}",
                hw.gpu_kind,
                HardwareTier::for_vram(free).label()
            );
            if let Some(r) = &recommended {
                println!("🎯 Recommended for your machine: {r}\n   pull it:  ollama pull {r}");
            }
            println!(
                "\nReviewed local-model catalog. Ollama-local entries can be pulled here;\n\
                 self-hosted entries require a separately managed local endpoint; cloud-only\n\
                 entries are shown only to prevent them being mistaken for offline models.\n"
            );

            for tier in [
                HardwareTier::Modest,
                HardwareTier::Single,
                HardwareTier::HighEnd,
            ] {
                println!("── {} ──", tier.label());
                for m in reg.catalog().filter(|m| m.tier == tier) {
                    let does_fit = fits.contains(&m.ollama_tag);
                    if fits_only && !does_fit {
                        continue;
                    }
                    let mut flags = Vec::new();
                    if m.installed {
                        flags.push("✓ installed".to_string());
                    }
                    flags.push(if does_fit {
                        "fits".to_string()
                    } else {
                        "needs more VRAM".to_string()
                    });
                    if !m.license.commercial_friendly() {
                        flags.push(format!("⚠ {}", m.license.spdx()));
                    }
                    if !m.can_pull_from_ollama() {
                        flags.push(m.local_availability.label().to_string());
                    }
                    if verify && m.can_pull_from_ollama() {
                        match verify_in_library(&m.ollama_tag).await {
                            Some(false) => flags.push("✗ not in library".to_string()),
                            None => flags.push("? unverified".to_string()),
                            Some(true) => {}
                        }
                    }
                    let identity = if m.ollama_tag.is_empty() {
                        m.source_ref.as_str()
                    } else {
                        m.ollama_tag.as_str()
                    };
                    println!(
                        "  {:30} {:11} {:10}  [{}]",
                        identity,
                        m.params,
                        m.license.spdx(),
                        flags.join(", ")
                    );
                }
                println!();
            }
            println!("Pull an Ollama-local tag, then select it in the chat panel or pass `--model <tag>`. Review each entry's caveat before a self-hosted deployment.");
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
                Some(m) => {
                    reject_non_ollama_catalog_model(&m)?;
                    m
                }
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

            let ollama = Arc::new(OllamaProvider::new(&config.ollama_url));
            let local_endpoints = LocalEndpointRegistry::from_config(&config)?;
            let target = select_cli_model(&config, &local_endpoints, &ollama, model).await?;
            let chosen_model = target.model.clone();

            let sentinel = VramSentinel::new(config.min_free_vram_mb, false);
            let hw = sentinel.detect_hardware().await;
            let num_ctx = effective_context_limit(hw.optimal_context, [&target]);

            let registry = ToolRegistry::with_defaults();
            eprintln!(
                "🔬 research: `{question}`\n   model: `{chosen_model}`  num_ctx: {}  tools: {}",
                num_ctx,
                registry.len()
            );
            if let Some(local) = &target.local {
                print_configured_local_target("research", local);
            }
            // Honest egress note: inference is local, but the web tools send
            // queries + fetched page text off the machine (mirrors the server
            // disclosure for the chat tools toggle).
            eprintln!(
                "   🌐 inference stays local, but search queries + fetched pages leave your \
                 machine (DuckDuckGo / Wikipedia / arXiv / fetched URLs)."
            );
            eprintln!();

            let mut agent = Agent::new(
                target.provider,
                registry,
                AgentConfig {
                    model: chosen_model,
                    num_ctx,
                    keep_alive: "1h".to_string(),
                    max_iterations,
                    system_suffix: rules_suffix.clone(),
                    replay_enabled: true,
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
            let installed: Vec<_> = ollama_for_pick
                .list_models()
                .await
                .unwrap_or_default()
                .into_iter()
                .filter(|model| is_offline_ollama_tag(&model.name))
                .collect();
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

        Commands::Plugins { action } => {
            let manager = PluginManager::new(knowledge_plugin_root())?;
            match action {
                PluginsAction::List => {
                    let installed = manager.list()?;
                    let installed_by_id: std::collections::BTreeMap<_, _> = installed
                        .iter()
                        .map(|manifest| (manifest.id.as_str(), manifest))
                        .collect();
                    println!("\n🧩 Curated GitHub knowledge plugins");
                    println!("   cache: {}", manager.install_root().display());
                    println!("   These are documentation-only references: Ollamax never clones, executes, installs, or registers repository code.\n");
                    for plugin in &manager.registry().plugins {
                        match installed_by_id.get(plugin.id.as_str()) {
                            Some(manifest) => println!(
                                "   ✓ {} — {}\n     {} · {} · pinned commit: {} · {} stars at install\n",
                                plugin.id,
                                plugin.name,
                                plugin.category,
                                manifest.repository.license,
                                manifest
                                    .repository
                                    .default_branch_commit_sha
                                    .as_deref()
                                    .unwrap_or("unavailable"),
                                manifest.repository.stars,
                            ),
                            None => println!(
                                "   ○ {} — {}\n     {} · requires {}+ stars · allowed licenses: {}\n",
                                plugin.id,
                                plugin.name,
                                plugin.category,
                                plugin.policy.minimum_stars,
                                plugin.policy.allowed_licenses.join(", "),
                            ),
                        }
                    }
                }
                PluginsAction::Install { id } => {
                    eprintln!("Fetching curated GitHub metadata and README for `{id}`…");
                    let manifest = manager.install(&id).await?;
                    println!(
                        "✅ installed `{}`\n   repository: {}\n   commit: {}\n   policy checked: {} stars (minimum {}), {}\n   saved: {} bytes of explicitly untrusted README documentation\n   cache: {}",
                        manifest.id,
                        manifest.repository.url,
                        manifest
                            .repository
                            .default_branch_commit_sha
                            .as_deref()
                            .unwrap_or("unavailable"),
                        manifest.repository.stars,
                        manifest.policy.minimum_stars,
                        manifest.repository.license,
                        manifest.document.bytes,
                        manager.install_root().join(&manifest.id).display(),
                    );
                }
                PluginsAction::Remove { id } => {
                    if manager.remove(&id)? {
                        println!("Removed knowledge plugin `{id}`.");
                    } else {
                        println!("Knowledge plugin `{id}` was not installed.");
                    }
                }
                PluginsAction::Context { query, max_plugins } => {
                    if query.is_empty() {
                        anyhow::bail!("forge plugins context: query is required");
                    }
                    let capped = max_plugins.clamp(1, MAX_RELEVANT_PLUGINS);
                    let contexts =
                        manager.load_relevant_context(&query.join(" "), capped, 12_000)?;
                    if contexts.is_empty() {
                        println!("No installed knowledge plugins match this query.");
                    } else {
                        println!(
                            "\nMatched {} untrusted knowledge plugin(s):",
                            contexts.len()
                        );
                        for context in &contexts {
                            println!(
                                "\n--- {} ({}, score {}) ---\n{}",
                                context.name, context.id, context.score, context.content
                            );
                        }
                    }
                }
            }
        }

        Commands::Eval { action } => match action {
            EvalAction::Validate { scenario } => {
                let scenario = load_scenario(&scenario)?;
                println!(
                    "✅ valid scenario `{}`\n   name: {}\n   allowed paths: {}\n   verifier: `{}`\n\nThis command only validates the declarative scenario; it does not run a model or verifier command.",
                    scenario.id,
                    scenario.name,
                    if scenario.allowed_paths.is_empty() {
                        "(none)".to_string()
                    } else {
                        scenario.allowed_paths.join(", ")
                    },
                    scenario.verify_command,
                );
            }
            EvalAction::Report { results, json } => {
                let records = JsonlEvaluationStore::new(&results).load()?;
                let report = score_records(&records);
                if json {
                    println!("{}", serde_json::to_string_pretty(&report)?);
                } else {
                    print_evaluation_score(&results, &report);
                }
            }
            EvalAction::Compare {
                baseline,
                candidate,
                json,
            } => {
                let baseline_records = JsonlEvaluationStore::new(&baseline).load()?;
                let candidate_records = JsonlEvaluationStore::new(&candidate).load()?;
                let comparison = compare_records(&baseline_records, &candidate_records);
                if json {
                    println!("{}", serde_json::to_string_pretty(&comparison)?);
                } else {
                    print_evaluation_comparison(&baseline, &candidate, &comparison);
                }
            }
        },

        Commands::Preload { model, keep_alive } => {
            let ollama = OllamaProvider::new(&config.ollama_url);
            let model = model.unwrap_or_else(|| config.default_model.clone());
            reject_non_ollama_catalog_model(&model)?;

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
                "`forge parallel` is not implemented in v0.2.1. \
                 Use `forge build` for parallel orchestration."
            );
        }

        Commands::Serve { port, host } => {
            ollama_forge::server::run(config, host, port).await?;
        }
    }

    Ok(())
}

/// Terminal approval gate for `forge agent`. The server/UI path has an
/// interactive diff preview; the CLI instead keeps the user in control with a
/// concise prompt before every consequential operation. File tools still enforce
/// their workspace sandbox even in `--autonomy auto` mode; the host shell is
/// separately guarded but not OS-sandboxed.
#[derive(Clone, Copy)]
struct CliApprovalPolicy {
    mode: AgentAutonomy,
}

#[async_trait::async_trait]
impl ApprovalPolicy for CliApprovalPolicy {
    fn requires_plan_approval(&self) -> bool {
        self.mode == AgentAutonomy::Confirm
    }

    async fn approve(&self, tool: &str, args: &serde_json::Value) -> Approval {
        match self.mode {
            AgentAutonomy::Auto => Approval::Allow,
            AgentAutonomy::Readonly => Approval::Deny,
            AgentAutonomy::Confirm => {
                let description = cli_action_description(tool, args);
                if cli_confirm(format!("Apply agent action: {description}? [y/N] ")).await {
                    Approval::Allow
                } else {
                    Approval::Deny
                }
            }
        }
    }

    async fn approve_plan(&self, plan: &str) -> Approval {
        if self.mode != AgentAutonomy::Confirm {
            return Approval::Allow;
        }
        let prompt =
            format!("\nAgent plan:\n{plan}\n\nRun this plan in the current workspace? [y/N] ");
        if cli_confirm(prompt).await {
            Approval::Allow
        } else {
            Approval::Deny
        }
    }
}

fn cli_action_description(tool: &str, args: &serde_json::Value) -> String {
    match tool {
        "fs_write" => {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("(unknown path)");
            let bytes = args
                .get("content")
                .and_then(|v| v.as_str())
                .map(str::len)
                .unwrap_or(0);
            format!("write {path} ({bytes} bytes)")
        }
        "fs_edit" => {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("(unknown path)");
            format!("edit {path}")
        }
        "shell" => {
            let command = args
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("(empty command)");
            format!("run `{command}`")
        }
        _ => format!("run {tool}"),
    }
}

async fn cli_confirm(prompt: String) -> bool {
    tokio::task::spawn_blocking(move || {
        use std::io::Write;
        let mut stderr = std::io::stderr().lock();
        if write!(stderr, "{prompt}")
            .and_then(|_| stderr.flush())
            .is_err()
        {
            return false;
        }
        let mut input = String::new();
        match std::io::stdin().read_line(&mut input) {
            Ok(_) => matches!(input.trim().to_ascii_lowercase().as_str(), "y" | "yes"),
            Err(_) => false,
        }
    })
    .await
    .unwrap_or(false)
}

/// Per-user cache location for provenance-recorded GitHub knowledge plugins.
/// This follows the same platform config convention as skills and rules; the
/// repository itself is never copied into the active workspace.
fn knowledge_plugin_root() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ollama-forge")
        .join("knowledge-plugins")
}

/// Select bounded, locally cached plugin documentation for a task. A broken
/// or tampered plugin cache is reported to the user and omitted rather than
/// becoming prompt context.
fn installed_plugin_context_suffix(query: &str) -> Result<(String, Vec<String>)> {
    let manager = PluginManager::new(knowledge_plugin_root())?;
    let contexts = manager.load_relevant_context(query, 3, 12_000)?;
    let ids = contexts.iter().map(|context| context.id.clone()).collect();
    Ok((render_context_suffix(&contexts), ids))
}

fn format_rate(rate: Option<f64>) -> String {
    rate.map(|value| format!("{:.1}%", value * 100.0))
        .unwrap_or_else(|| "n/a".to_string())
}

fn format_median(value: Option<u64>, unit: &str) -> String {
    value
        .map(|value| format!("{value} {unit}"))
        .unwrap_or_else(|| "n/a".to_string())
}

fn print_evaluation_score(path: &std::path::Path, report: &ScoreReport) {
    println!(
        "\n📊 Local evaluation report\n   results: {}",
        path.display()
    );
    println!(
        "   runs: {} · verified: {}/{} ({})",
        report.total_runs,
        report.verified_completions,
        report.total_runs,
        format_rate(report.verified_completion_rate),
    );
    println!(
        "   checks: build {} · lint {} · tests {}",
        format_rate(report.build_pass_rate),
        format_rate(report.lint_pass_rate),
        format_rate(report.test_pass_rate),
    );
    println!(
        "   medians: {} · {} total tokens · {} model calls · {} tool calls",
        format_median(report.median_duration_ms, "ms"),
        format_median(report.median_total_tokens, ""),
        format_median(report.median_model_calls, ""),
        format_median(report.median_tool_calls, ""),
    );
    println!(
        "   safety evidence: regressions {} · scope-violation runs {} ({} paths)",
        format_rate(report.regression_rate),
        format_rate(report.scope_violation_rate),
        report.scope_violation_count,
    );
}

fn print_evaluation_comparison(
    baseline_path: &std::path::Path,
    candidate_path: &std::path::Path,
    comparison: &ScoreComparison,
) {
    println!("\n📊 Local evaluation comparison");
    println!("   baseline:  {}", baseline_path.display());
    println!("   candidate: {}", candidate_path.display());
    println!(
        "   verified completion: {} → {} ({:+.1} pp)",
        format_rate(comparison.baseline.verified_completion_rate),
        format_rate(comparison.candidate.verified_completion_rate),
        comparison.delta.verified_completion_rate.unwrap_or(0.0) * 100.0,
    );
    println!(
        "   test pass rate:       {} → {} ({:+.1} pp)",
        format_rate(comparison.baseline.test_pass_rate),
        format_rate(comparison.candidate.test_pass_rate),
        comparison.delta.test_pass_rate.unwrap_or(0.0) * 100.0,
    );
    println!(
        "   median duration:      {} → {} ({:+} ms)",
        format_median(comparison.baseline.median_duration_ms, "ms"),
        format_median(comparison.candidate.median_duration_ms, "ms"),
        comparison.delta.median_duration_ms.unwrap_or(0),
    );
    println!(
        "   median total tokens:  {} → {} ({:+})",
        format_median(comparison.baseline.median_total_tokens, ""),
        format_median(comparison.candidate.median_total_tokens, ""),
        comparison.delta.median_total_tokens.unwrap_or(0),
    );
    println!(
        "   regression rate:      {} → {} ({:+.1} pp)",
        format_rate(comparison.baseline.regression_rate),
        format_rate(comparison.candidate.regression_rate),
        comparison.delta.regression_rate.unwrap_or(0.0) * 100.0,
    );
    println!(
        "   scope violation rate: {} → {} ({:+.1} pp)",
        format_rate(comparison.baseline.scope_violation_rate),
        format_rate(comparison.candidate.scope_violation_rate),
        comparison.delta.scope_violation_rate.unwrap_or(0.0) * 100.0,
    );
}

/// One CLI model selection. A configured endpoint stays explicitly local: the
/// public `local:<endpoint>/<model>` selector is kept in prompts, plans, and
/// replay records while the registry rewrites it to the endpoint's declared
/// served model only at the provider boundary.
#[derive(Clone)]
struct CliModelTarget {
    model: String,
    provider: Arc<dyn LlmProvider>,
    local: Option<ConfiguredLocalModelMetadata>,
}

/// Treat an absent, blank, or `auto` CLI flag as an automatic choice. Keep
/// every other string intact so the strict local-selector parser can reject
/// misleading whitespace rather than silently changing a user selection.
fn explicit_cli_model(requested: Option<String>) -> Option<String> {
    requested.filter(|model| !model.trim().is_empty() && !model.eq_ignore_ascii_case("auto"))
}

/// Resolve one explicit model name. Ordinary model names continue through
/// Ollama unchanged; only the reserved `local:` namespace may use a separately
/// configured loopback endpoint.
fn resolve_named_cli_model(
    local_endpoints: &LocalEndpointRegistry,
    ollama: &Arc<OllamaProvider>,
    model: String,
) -> Result<CliModelTarget> {
    if let Some(configured) = local_endpoints.try_resolve(&model)? {
        let local = configured.metadata();
        return Ok(CliModelTarget {
            model: local.selector.clone(),
            provider: configured.provider,
            local: Some(local),
        });
    }
    reject_non_ollama_catalog_model(&model)?;
    let provider: Arc<dyn LlmProvider> = ollama.clone();
    Ok(CliModelTarget {
        model,
        provider,
        local: None,
    })
}

/// Catalog entries that need a separate server—or are cloud-only—must never
/// become ordinary Ollama requests by accident. Self-hosted models enter
/// through the separate `local:` namespace, and cloud-only entries are
/// rejected before any provider is called.
fn reject_non_ollama_catalog_model(model: &str) -> Result<()> {
    use ollama_forge::models::{is_ollama_cloud_tag, LocalAvailability, ModelRegistry};

    let registry = ModelRegistry::seed();
    // This guard is deliberately more forgiving than the `local:` parser:
    // a malformed local selector must fail strictly, while a cloud/server
    // catalog tag with incidental surrounding whitespace still must not slip
    // through to Ollama.
    let lookup = model.trim();
    if parse_local_model_selector(lookup)?.is_some() {
        anyhow::bail!(
            "`{model}` is a configured local endpoint selector, not an Ollama tag. This workflow only supports pulled local Ollama artifacts; use `forge agent`, `forge team`, or the desktop Agent/Team workflow for configured endpoint routing."
        );
    }
    if is_ollama_cloud_tag(lookup) {
        anyhow::bail!(
            "`{model}` is a cloud-tagged model and cannot be used as an offline Ollama model. Configure a separately operated loopback endpoint and select a `local:<endpoint>/<model>` selector only for a model you actually self-host."
        );
    }
    let exact = |candidate: &ollama_forge::models::CuratedModel| {
        candidate.ollama_tag.eq_ignore_ascii_case(lookup)
            || candidate.source_ref.eq_ignore_ascii_case(lookup)
            || candidate
                .installed_aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(lookup))
    };
    let compact_model = compact_catalog_identifier(lookup);
    let candidate = registry
        .catalog()
        .find(|candidate| {
            candidate.local_availability != LocalAvailability::OllamaLocal && exact(candidate)
        })
        .or_else(|| {
            registry.catalog().find(|candidate| {
                candidate.local_availability != LocalAvailability::OllamaLocal
                    && compact_catalog_identifier(&candidate.family) == compact_model
            })
        });

    if let Some(candidate) = candidate {
        match candidate.local_availability {
            LocalAvailability::CloudOnly => anyhow::bail!(
                "`{model}` is cataloged as cloud-only and cannot be used as an offline Ollama model. {}",
                candidate.caveat
            ),
            LocalAvailability::SelfHostedLocal => anyhow::bail!(
                "`{model}` is a separately self-hosted catalog model, not an Ollama tag. Configure a loopback endpoint and select it as `local:<endpoint>/<model>`. {}",
                candidate.caveat
            ),
            LocalAvailability::OllamaLocal => {}
        }
    }
    Ok(())
}

/// Compare a human-facing model family name with common direct spellings such
/// as `DeepSeek-V4-Flash`, without treating punctuation/case as a separate
/// runtime. This only runs after exact catalog identity checks and only for
/// entries that are explicitly not Ollama-local.
fn compact_catalog_identifier(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Select a model for a user-facing CLI workflow. `local:` selections are
/// opt-in: they must name a configured endpoint. Auto selection deliberately
/// remains Ollama-only unless the configured default itself is an explicit
/// configured-local selector.
async fn select_cli_model(
    config: &Config,
    local_endpoints: &LocalEndpointRegistry,
    ollama: &Arc<OllamaProvider>,
    requested: Option<String>,
) -> Result<CliModelTarget> {
    let model = if let Some(model) = explicit_cli_model(requested) {
        model
    } else if parse_local_model_selector(&config.default_model)?.is_some() {
        config.default_model.clone()
    } else {
        pick_installed_model(config, ollama).await?
    };
    resolve_named_cli_model(local_endpoints, ollama, model)
}

/// Honor an endpoint operator's declared context ceiling while retaining the
/// machine-derived local budget. This is a conservative request bound, not a
/// claim that a self-hosted model fits the current GPU.
fn effective_context_limit<'a>(
    hardware_limit: usize,
    targets: impl IntoIterator<Item = &'a CliModelTarget>,
) -> usize {
    targets
        .into_iter()
        .filter_map(|target| {
            target
                .local
                .as_ref()
                .and_then(|local| local.context_window_tokens)
                .filter(|limit| *limit > 0)
        })
        .fold(hardware_limit.max(1), usize::min)
}

/// Make the local-only boundary visible whenever a command uses a configured
/// endpoint. No secret or bearer-token environment variable is rendered.
fn print_configured_local_target(role: &str, local: &ConfiguredLocalModelMetadata) {
    let label = local.label.as_deref().unwrap_or(&local.served_model);
    eprintln!(
        "   {role}: `{}` → configured loopback endpoint `{}` (served as `{label}`, request cap {})",
        local.selector, local.endpoint_url, local.max_parallel_requests,
    );
}

/// The old scout heuristic validates installed Ollama models. If a writer is
/// configured-local, avoid a surprise fallback to Ollama: reuse that bounded
/// local provider unless the user explicitly selects a scout model.
async fn pick_team_scout_target(
    config: &Config,
    local_endpoints: &LocalEndpointRegistry,
    ollama: &Arc<OllamaProvider>,
    writer: &CliModelTarget,
    requested: Option<String>,
) -> Result<CliModelTarget> {
    if let Some(model) = explicit_cli_model(requested) {
        return resolve_named_cli_model(local_endpoints, ollama, model);
    }
    if writer.local.is_some() {
        return Ok(writer.clone());
    }
    let model = pick_team_scout_model(config, ollama, &writer.model, None).await?;
    resolve_named_cli_model(local_endpoints, ollama, model)
}

/// Prefer an explicitly selected/configured local planning model when one is
/// declared. For a configured-local writer, the safe fallback is that same
/// provider—not an implicit Ollama call—so endpoint-only setups remain usable.
async fn pick_team_planner_target(
    config: &Config,
    local_endpoints: &LocalEndpointRegistry,
    ollama: &Arc<OllamaProvider>,
    writer: &CliModelTarget,
    requested: Option<String>,
) -> Result<CliModelTarget> {
    if let Some(model) = explicit_cli_model(requested) {
        return resolve_named_cli_model(local_endpoints, ollama, model);
    }
    if parse_local_model_selector(&config.planning_model)?.is_some() {
        return resolve_named_cli_model(local_endpoints, ollama, config.planning_model.clone());
    }
    if writer.local.is_some() {
        return Ok(writer.clone());
    }
    let model = pick_team_planner_model(config, ollama, &writer.model, None).await?;
    resolve_named_cli_model(local_endpoints, ollama, model)
}

/// The reviewer defaults to the writer model. A different reviewer can be an
/// explicitly configured local selector or an ordinary Ollama model.
fn pick_team_reviewer_target(
    local_endpoints: &LocalEndpointRegistry,
    ollama: &Arc<OllamaProvider>,
    writer: &CliModelTarget,
    requested: Option<String>,
) -> Result<CliModelTarget> {
    match explicit_cli_model(requested) {
        Some(model) => resolve_named_cli_model(local_endpoints, ollama, model),
        None => Ok(writer.clone()),
    }
}

/// `forge agent "..."` — a workspace-aware local coding loop for terminal
/// users. Unlike `forge chat`, this path gives the model concrete filesystem
/// discovery and edit tools rooted at the current directory. It intentionally
/// registers no web tools, so code-agent inference and tool use stay local.
async fn run_workspace_agent(
    config: &Config,
    rules_suffix: &str,
    task: String,
    requested_model: Option<String>,
    max_iterations: usize,
    autonomy: AgentAutonomy,
) -> Result<()> {
    let workspace = std::env::current_dir()?
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("resolve current workspace: {e}"))?;
    let ollama = Arc::new(OllamaProvider::new(&config.ollama_url));
    let local_endpoints = LocalEndpointRegistry::from_config(config)?;
    let target = select_cli_model(config, &local_endpoints, &ollama, requested_model).await?;
    let model = target.model.clone();
    let sentinel = VramSentinel::new(config.min_free_vram_mb, false);
    let hardware = sentinel.detect_hardware().await;
    let num_ctx = effective_context_limit(hardware.optimal_context, [&target]);

    let plugin_suffix = match installed_plugin_context_suffix(&task) {
        Ok((suffix, ids)) => {
            if !ids.is_empty() {
                eprintln!(
                    "   knowledge plugins: {} (untrusted reference only)",
                    ids.join(", ")
                );
            }
            suffix
        }
        Err(error) => {
            eprintln!("   ⚠️  installed knowledge plugins were not loaded: {error:#}");
            String::new()
        }
    };

    let mut registry = ToolRegistry::new();
    let workspace_fs = WorkspaceFs::new(&workspace);
    registry.register(Arc::new(FsListTool::from_workspace(workspace_fs.clone())));
    registry.register(Arc::new(FsSearchTool::from_workspace(workspace_fs.clone())));
    registry.register(Arc::new(FsReadTool::from_workspace(workspace_fs.clone())));
    registry.register(Arc::new(FsWriteTool::from_workspace(workspace_fs.clone())));
    registry.register(Arc::new(FsEditTool::from_workspace(workspace_fs.clone())));
    registry.register(Arc::new(ShellTool::from_workspace(
        &workspace,
        workspace_fs,
        ShellPolicy::default(),
    )));

    let mut system_suffix = format!(
        "{rules_suffix}\n\n## Workspace agent\nYou are working inside `{}`. Use fs_list and fs_search to orient yourself, then read files before editing. Keep every path relative to this workspace. Use fs_write/fs_edit to make requested code changes rather than only returning a code block. The user must approve consequential actions unless autonomy is auto.",
        workspace.display()
    );
    system_suffix.push_str(&plugin_suffix);
    if system_suffix.starts_with("\n\n") {
        system_suffix = system_suffix.trim_start().to_string();
    }

    eprintln!(
        "⚒  forge agent\n   workspace: {}\n   model: `{model}` · num_ctx: {} · autonomy: {:?}\n",
        workspace.display(),
        num_ctx,
        autonomy,
    );
    if let Some(local) = &target.local {
        print_configured_local_target("agent", local);
    }
    let approval: Arc<dyn ApprovalPolicy> = Arc::new(CliApprovalPolicy { mode: autonomy });
    let mut agent = Agent::new(
        target.provider,
        registry,
        AgentConfig {
            model,
            num_ctx,
            keep_alive: "1h".to_string(),
            max_iterations: max_iterations.max(1),
            system_suffix,
            replay_enabled: true,
        },
    )
    .with_approval(approval)
    .with_planning(autonomy == AgentAutonomy::Confirm);

    let trace = agent
        .run(&task, |step| {
            let preview: String = step
                .result_preview
                .replace('\n', " ")
                .chars()
                .take(240)
                .collect();
            eprintln!(
                "   [{}] {} {}",
                if step.ok { "ok" } else { "failed" },
                step.tool,
                preview
            );
        })
        .await?;
    if trace.iteration_capped {
        eprintln!(
            "⚠️  agent reached the {}-round safety limit.",
            max_iterations.max(1)
        );
    }
    println!("{}", trace.answer);
    Ok(())
}

struct WorkspaceTeamRequest {
    task: String,
    requested_model: Option<String>,
    requested_scout_model: Option<String>,
    requested_planner_model: Option<String>,
    reviewer_model: Option<String>,
    max_iterations: usize,
    max_repair_rounds: usize,
    mode: TeamMode,
    autonomy: AgentAutonomy,
}

/// `forge team "..."` — a bounded local coding-team workflow. The only
/// parallel lane is optional read-only reconnaissance; a single implementer
/// owns workspace changes, and repository-detected verification commands are
/// gated through the same autonomy policy as ordinary agent shell actions.
async fn run_workspace_team(
    config: &Config,
    rules_suffix: &str,
    request: WorkspaceTeamRequest,
) -> Result<()> {
    let WorkspaceTeamRequest {
        task,
        requested_model,
        requested_scout_model,
        requested_planner_model,
        reviewer_model,
        max_iterations,
        max_repair_rounds,
        mode,
        autonomy,
    } = request;
    let bounded_iterations = max_iterations.clamp(1, ollama_forge::team::MAX_TEAM_ITERATIONS);
    let bounded_repairs = max_repair_rounds.min(ollama_forge::team::MAX_TEAM_REPAIR_ROUNDS);
    if bounded_iterations != max_iterations || bounded_repairs != max_repair_rounds {
        eprintln!(
            "   ⚠️  team budgets were capped at {} tool rounds and {} repair round(s).",
            ollama_forge::team::MAX_TEAM_ITERATIONS,
            ollama_forge::team::MAX_TEAM_REPAIR_ROUNDS,
        );
    }
    let workspace = std::env::current_dir()?
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("resolve current workspace: {e}"))?;
    let ollama = Arc::new(OllamaProvider::new(&config.ollama_url));
    let local_endpoints = LocalEndpointRegistry::from_config(config)?;
    let writer = select_cli_model(config, &local_endpoints, &ollama, requested_model).await?;
    let scout = pick_team_scout_target(
        config,
        &local_endpoints,
        &ollama,
        &writer,
        requested_scout_model,
    )
    .await?;
    let planner = pick_team_planner_target(
        config,
        &local_endpoints,
        &ollama,
        &writer,
        requested_planner_model,
    )
    .await?;
    let reviewer = pick_team_reviewer_target(&local_endpoints, &ollama, &writer, reviewer_model)?;
    let model = writer.model.clone();
    let scout_model = scout.model.clone();
    let planner_model = planner.model.clone();
    let reviewer_model = reviewer.model.clone();
    let sentinel = VramSentinel::new(config.min_free_vram_mb, false);
    let hardware = sentinel.detect_hardware().await;
    let num_ctx = effective_context_limit(
        hardware.optimal_context,
        [&writer, &scout, &planner, &reviewer],
    );
    let plugin_suffix = match installed_plugin_context_suffix(&task) {
        Ok((suffix, ids)) => {
            if !ids.is_empty() {
                eprintln!(
                    "   knowledge plugins: {} (untrusted reference only)",
                    ids.join(", ")
                );
            }
            suffix
        }
        Err(error) => {
            eprintln!("   ⚠️  installed knowledge plugins were not loaded: {error:#}");
            String::new()
        }
    };
    let system_suffix = format!(
        "{rules_suffix}\n\n## Local team contract\nThis team has read-only scouts, one controlled writer, fixed repository-detected verification commands, and an advisory reviewer. Do not claim a task is verified unless the verifier reports a pass."
    ) + &plugin_suffix;
    let coordinator = TeamCoordinator::new_with_providers(
        TeamProviders::new(
            scout.provider.clone(),
            planner.provider.clone(),
            writer.provider.clone(),
            reviewer.provider.clone(),
        ),
        &workspace,
        TeamConfig {
            model: model.clone(),
            scout_model: Some(scout_model.clone()),
            planner_model: Some(planner_model.clone()),
            reviewer_model: Some(reviewer_model.clone()),
            num_ctx,
            keep_alive: "1h".to_string(),
            max_iterations: bounded_iterations,
            max_repair_rounds: bounded_repairs,
            mode,
            system_suffix,
            replay_enabled: true,
        },
    )?;
    eprintln!(
        "⚒  forge team\n   workspace: {}\n   writer: `{model}` · scouts: `{scout_model}` · planner: `{planner_model}` · num_ctx: {} · mode: {:?} · autonomy: {:?}\n",
        workspace.display(),
        num_ctx,
        mode,
        autonomy,
    );
    for (role, target) in [
        ("team writer", &writer),
        ("team scouts", &scout),
        ("team planner", &planner),
        ("team reviewer", &reviewer),
    ] {
        if let Some(local) = &target.local {
            print_configured_local_target(role, local);
        }
    }
    if mode == TeamMode::ParallelScouts {
        eprintln!(
            "   note: only read-only scouts run concurrently; the writer remains single-lane."
        );
    }
    let approval: Arc<dyn ApprovalPolicy> = Arc::new(CliApprovalPolicy { mode: autonomy });
    let run = coordinator
        .run(&task, approval, |event| match event {
            TeamEvent::PlanCreated { plan } => {
                if plan.verification_commands.is_empty() {
                    eprintln!("   ⚠️  no conventional verifier was detected; status cannot be verified automatically.");
                } else {
                    eprintln!(
                        "   plan: one writer `{}` · scouts `{}` · planner `{}` · reviewer `{}` · verifiers: {}",
                        plan.writer_model,
                        plan.scout_model,
                        plan.planner_model,
                        plan.reviewer_model,
                        plan.verification_commands.join("; ")
                    );
                }
            }
            TeamEvent::ScoutStarted { role } => eprintln!("   ⏳ scout start  {role:?}"),
            TeamEvent::ScoutFinished { role, steps } => {
                eprintln!("   ✅ scout done   {role:?} ({steps} tool steps)")
            }
            TeamEvent::PlannerStarted => eprintln!("   ⏳ planner start read-only synthesis"),
            TeamEvent::PlannerFinished { .. } => eprintln!("   ✅ planner done  read-only hand-off"),
            TeamEvent::ImplementerStarted { repair_round } => {
                let label = if *repair_round == 0 { "implementation" } else { "repair" };
                eprintln!("   ⏳ writer start {label} pass {repair_round}")
            }
            TeamEvent::ImplementerStep { step, .. } => {
                let preview: String = step
                    .result_preview
                    .replace('\n', " ")
                    .chars()
                    .take(180)
                    .collect();
                eprintln!(
                    "   [{}] writer {} {}",
                    if step.ok { "ok" } else { "failed" },
                    step.tool,
                    preview
                );
            }
            TeamEvent::ImplementerFinished { repair_round, steps } => {
                eprintln!("   ✅ writer done  pass {repair_round} ({steps} tool steps)")
            }
            TeamEvent::VerificationStarted { command } => {
                eprintln!("   ⏳ verify       `{command}`")
            }
            TeamEvent::VerificationFinished { result } => {
                let mark = if result.passed { "✅" } else { "❌" };
                let suffix = if result.skipped_by_user { " (declined)" } else { "" };
                eprintln!("   {mark} verify       `{}`{suffix}", result.command);
            }
            TeamEvent::ReviewerFinished { available } => {
                if *available {
                    eprintln!("   ✅ reviewer done")
                } else {
                    eprintln!("   ⚠️  reviewer unavailable; inspect the recorded review warning")
                }
            }
        })
        .await?;

    let status = match run.status {
        TeamStatus::Verified => "✅ verified",
        TeamStatus::ChecksPassed => "ℹ️  checks passed; human acceptance still needed",
        TeamStatus::NeedsAttention => "⚠️  needs attention",
        TeamStatus::VerificationDeclined => "⚠️  verification declined",
        TeamStatus::PlanDeclined => "⚠️  implementation plan declined",
    };
    eprintln!(
        "\n{status} · {} writer mutation step(s) · {} ms",
        run.writer_mutation_steps, run.elapsed_ms
    );
    if !run.implementation_answers.is_empty() {
        println!("{}", run.implementation_answers.join("\n\n"));
    }
    if !run.review.trim().is_empty() {
        println!("\nReview:\n{}", run.review);
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

/// If `FORGE_REPLAY_LOG` is set, append a record for this local-provider call
/// to it. Best-effort: failures here only log a warning, never break the
/// user's command. The log is opt-in because not every user wants prompts
/// persisted to disk.
async fn maybe_log_replay(opts: &GenerateOptions, response: &str, provider: &dyn LlmProvider) {
    let Ok(path) = std::env::var("FORGE_REPLAY_LOG") else {
        return;
    };
    // Best effort: Ollama supplies a manifest digest, while a separately
    // operated OpenAI-compatible server may not expose a stable artifact ID.
    // An empty value remains an honest "not available" marker rather than a
    // fabricated hash for a remote model server.
    let digest = provider
        .model_fingerprint(&opts.model)
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
url = "http://127.0.0.1:11434"
default_model = "qwen3.5:4b"
planning_model = "deepseek-r1:8b"
execution_models = ["qwen3.5:4b", "deepseek-r1:8b", "qwen3.5:9b", "gemma4:12b"]

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

/// Pick the most-capable installed Ollama model. Prefers `config.default_model`
/// if it's installed; otherwise falls back to the largest installed model;
/// otherwise errors.
async fn pick_installed_model(config: &Config, ollama: &OllamaProvider) -> Result<String> {
    reject_non_ollama_catalog_model(&config.default_model)?;
    let installed: Vec<_> = ollama
        .list_models()
        .await
        .map_err(|e| anyhow::anyhow!("could not list ollama models: {e}"))?
        .into_iter()
        .filter(|model| is_offline_ollama_tag(&model.name))
        .collect();
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

/// Assign a read-only scout model without silently sending Ollama an
/// unavailable tag. A user-requested scout model wins. Otherwise, only an
/// installed model from `execution_models` that is no larger than the writer
/// is selected; this makes the existing model ladder useful for inexpensive
/// repository reconnaissance while preserving the writer's stronger model.
async fn pick_team_scout_model(
    config: &Config,
    ollama: &OllamaProvider,
    writer_model: &str,
    requested: Option<String>,
) -> Result<String> {
    let requested = requested.filter(|model| !model.trim().is_empty());
    let installed: Vec<_> = match ollama.list_models().await {
        Ok(models) => models
            .into_iter()
            .filter(|model| is_offline_ollama_tag(&model.name))
            .collect(),
        Err(error) if requested.is_none() => {
            eprintln!(
                "   ⚠️  could not inspect installed models for a scout role; using writer model `{writer_model}`: {error:#}"
            );
            return Ok(writer_model.to_string());
        }
        Err(error) => {
            return Err(anyhow::anyhow!(
                "could not list Ollama models to validate requested scout model: {error:#}"
            ))
        }
    };

    if let Some(requested) = requested {
        if installed.iter().any(|model| model.name == requested) {
            return Ok(requested);
        }
        anyhow::bail!(
            "requested scout model `{requested}` is not installed. Run `ollama list` or omit --scout-model."
        );
    }

    let writer_size = installed
        .iter()
        .find(|model| model.name == writer_model)
        .map(|model| model.size);
    let candidate = config
        .execution_models
        .iter()
        .filter(|name| name.as_str() != writer_model)
        .filter_map(|name| installed.iter().find(|model| model.name == *name))
        .filter(|model| writer_size.map_or(true, |writer_size| model.size <= writer_size))
        // Prefer the strongest smaller model rather than arbitrarily choosing
        // the tiniest one: scouts still have to understand an unfamiliar repo.
        .max_by_key(|model| model.size);
    Ok(candidate
        .map(|model| model.name.clone())
        .unwrap_or_else(|| writer_model.to_string()))
}

/// Resolve the read-only planning/synthesis model. A configured planning model
/// is useful only when it is actually installed; otherwise the writer model is
/// safer than failing late or silently sending an unavailable tag to Ollama.
async fn pick_team_planner_model(
    config: &Config,
    ollama: &OllamaProvider,
    writer_model: &str,
    requested: Option<String>,
) -> Result<String> {
    let requested = requested.filter(|model| !model.trim().is_empty());
    let configured = (!config.planning_model.trim().is_empty()
        && config.planning_model != writer_model)
        .then(|| config.planning_model.clone());
    let candidate = requested.clone().or(configured);
    let Some(candidate) = candidate else {
        return Ok(writer_model.to_string());
    };
    match ollama.list_models().await {
        Ok(installed)
            if installed
                .iter()
                .filter(|model| is_offline_ollama_tag(&model.name))
                .any(|model| model.name == candidate) =>
        {
            Ok(candidate)
        }
        Ok(_) if requested.is_some() => anyhow::bail!(
            "requested planner model `{candidate}` is not installed. Run `ollama list` or omit --planner-model."
        ),
        Ok(_) => {
            eprintln!(
                "   ⚠️  configured planning model `{candidate}` is not installed; using writer model `{writer_model}` for planning."
            );
            Ok(writer_model.to_string())
        }
        Err(error) if requested.is_some() => Err(anyhow::anyhow!(
            "could not list Ollama models to validate requested planner model: {error:#}"
        )),
        Err(error) => {
            eprintln!(
                "   ⚠️  could not inspect configured planning model; using writer model `{writer_model}`: {error:#}"
            );
            Ok(writer_model.to_string())
        }
    }
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

#[cfg(test)]
mod cli_local_endpoint_tests {
    use super::{
        effective_context_limit, pick_team_planner_target, resolve_named_cli_model,
        select_cli_model,
    };
    use ollama_forge::providers::{LocalEndpointRegistry, OllamaProvider};
    use ollama_forge::{Config, LocalEndpointConfig, LocalEndpointModelConfig};
    use std::sync::Arc;

    fn endpoint_config() -> LocalEndpointConfig {
        LocalEndpointConfig {
            id: "lab".to_string(),
            url: "http://localhost:8010".to_string(),
            api_key_env: None,
            max_parallel_requests: 2,
            models: vec![
                LocalEndpointModelConfig {
                    id: "writer".to_string(),
                    served_model: "MiniMax-M3".to_string(),
                    label: Some("Local writer".to_string()),
                    vision: false,
                    thinking: true,
                    context_window_tokens: Some(32_768),
                },
                LocalEndpointModelConfig {
                    id: "planner".to_string(),
                    served_model: "DeepSeek-V4-Flash".to_string(),
                    label: None,
                    vision: false,
                    thinking: true,
                    context_window_tokens: Some(8_192),
                },
            ],
        }
    }

    fn configured() -> (Config, LocalEndpointRegistry, Arc<OllamaProvider>) {
        let config = Config {
            local_endpoints: vec![endpoint_config()],
            ..Config::default()
        };
        let registry = LocalEndpointRegistry::from_config(&config).unwrap();
        let ollama = Arc::new(OllamaProvider::new(&config.ollama_url));
        (config, registry, ollama)
    }

    #[test]
    fn explicit_selector_is_resolved_without_contacting_ollama() {
        let (_config, registry, ollama) = configured();
        let target =
            resolve_named_cli_model(&registry, &ollama, "local:lab/writer".to_string()).unwrap();

        let local = target.local.as_ref().expect("configured local metadata");
        assert_eq!(target.model, "local:lab/writer");
        assert_eq!(local.served_model, "MiniMax-M3");
        assert_eq!(local.endpoint_url, "http://127.0.0.1:8010/v1");
        assert_eq!(target.provider.name(), "openai-compatible-local:lab");
    }

    #[tokio::test]
    async fn configured_default_and_planner_keep_endpoint_only_team_local() {
        let (mut config, registry, ollama) = configured();
        config.default_model = "local:lab/writer".to_string();
        config.planning_model = "local:lab/planner".to_string();

        let writer = select_cli_model(&config, &registry, &ollama, None)
            .await
            .unwrap();
        let planner = pick_team_planner_target(&config, &registry, &ollama, &writer, None)
            .await
            .unwrap();

        assert_eq!(writer.model, "local:lab/writer");
        assert_eq!(planner.model, "local:lab/planner");
        assert_eq!(writer.provider.name(), "openai-compatible-local:lab");
        assert_eq!(planner.provider.name(), "openai-compatible-local:lab");
        assert_eq!(effective_context_limit(16_384, [&writer, &planner]), 8_192,);
    }

    #[test]
    fn unknown_local_selector_fails_closed_instead_of_falling_back_to_ollama() {
        let (_config, registry, ollama) = configured();
        let error = match resolve_named_cli_model(
            &registry,
            &ollama,
            "local:lab/not-configured".to_string(),
        ) {
            Ok(_) => panic!("unknown local selector must not fall back to Ollama"),
            Err(error) => error,
        };
        assert!(format!("{error:#}").contains("was not found"));
    }

    #[test]
    fn cloud_only_catalog_tag_is_rejected_before_any_ollama_request() {
        let (_config, registry, ollama) = configured();
        for selector in [
            "minimax-m3:cloud",
            "  MINIMAX-M3:CLOUD  ",
            "  QWEN3.5:CLOUD  ",
            "gemma4:31b-cloud",
        ] {
            let error = match resolve_named_cli_model(&registry, &ollama, selector.to_string()) {
                Ok(_) => panic!("cloud-only catalog model must not be routed to Ollama"),
                Err(error) => error,
            };
            let rendered = format!("{error:#}");
            assert!(
                rendered.contains("cloud-only") || rendered.contains("cloud-tagged"),
                "error: {rendered}"
            );
        }
    }

    #[test]
    fn self_hosted_catalog_family_requires_an_explicit_local_selector() {
        let (_config, registry, ollama) = configured();
        for selector in ["DeepSeek-V4-Flash", "deepseek-ai/DEEPSEEK-V4-FLASH"] {
            let error = match resolve_named_cli_model(&registry, &ollama, selector.to_string()) {
                Ok(_) => panic!("self-hosted catalog model must not be routed to Ollama"),
                Err(error) => error,
            };
            let rendered = format!("{error:#}");
            assert!(rendered.contains("self-hosted"));
            assert!(rendered.contains("local:<endpoint>/<model>"));
        }
    }
}
