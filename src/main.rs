use anyhow::Result;
use clap::Parser;
use ollama_forge::cli::{Cli, Commands, SkillsAction};
use ollama_forge::orchestrator::{BuildRequest, Orchestrator, OrchestratorConfig};
use ollama_forge::providers::{GenerateOptions, LlmProvider, OllamaProvider};
use ollama_forge::security::{SecurityGuard, Severity};
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

    match cli.command {
        Commands::Init { force } => init_project(force).await?,

        Commands::Build { task, no_security } => {
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
            })
            .await?;

            // Surface progress to stderr so the user knows the orchestrator
            // is alive during a long parallel run. The actual generated
            // artifact still goes to stdout so it can be piped to a file.
            eprintln!("🏗  forge build: orchestrating…");
            let result = orchestrator.execute(request).await?;
            eprintln!("✅ build completed on `{}`", result.model_used);
            println!("{}", result.output);
        }

        Commands::Chat { model, prompt } => {
            let ollama = OllamaProvider::new(&config.ollama_url);
            let opts = GenerateOptions {
                model: model.unwrap_or_else(|| config.default_model.clone()),
                prompt: prompt.unwrap_or_default(),
                stream: true,
                ..Default::default()
            };
            // Real streaming: print each token as it arrives. Previously we
            // claimed `stream: true` but `generate()` buffered the whole
            // response, giving the user a 20s wait for a wall of text.
            use std::io::Write;
            let mut stdout = std::io::stdout().lock();
            let bytes = ollama
                .generate_streaming(opts, |chunk| {
                    let _ = stdout.write_all(chunk.as_bytes());
                    let _ = stdout.flush();
                })
                .await?;
            // Trailing newline so the next prompt isn't glued to the response.
            let _ = writeln!(stdout);
            if bytes == 0 {
                eprintln!("forge: model returned no tokens");
            }
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

            // Build the prompt: skill system prompt + planning hint + the task.
            let mut system_prompt = skill.prompts.system.clone();
            if let Some(planning) = &skill.prompts.planning {
                system_prompt.push_str("\n\nPlanning guidance: ");
                system_prompt.push_str(planning);
            }
            if let Some(execution) = &skill.prompts.execution {
                system_prompt.push_str("\n\nExecution guidance: ");
                system_prompt.push_str(execution);
            }

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
            run_analyze(&config, path, analysis_type).await?;
        }

        Commands::Test { path, framework } => {
            run_test_gen(&config, path, framework).await?;
        }

        Commands::Parallel { .. } => {
            anyhow::bail!(
                "`forge parallel` is not implemented in v0.1.0. \
                 Use `forge build` for parallel orchestration."
            );
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

        let opts = GenerateOptions {
            model,
            prompt: format!(
                "Review the following code. Focus on: {kind_label}.\n\
                 List the top 5 issues with file:line references.\n\
                 Be concrete; do not invent issues that aren't there.\n\n\
                 {combined}"
            ),
            system: Some(
                "You are a senior code reviewer. Output a numbered list of \
                 concrete issues. If no issues exist, say so explicitly."
                    .to_string(),
            ),
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
async fn run_test_gen(config: &Config, path: PathBuf, framework: Option<String>) -> Result<()> {
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
        system: Some(
            "You are a senior test engineer. Write production-quality tests \
             that compile and run on the first try."
                .to_string(),
        ),
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
