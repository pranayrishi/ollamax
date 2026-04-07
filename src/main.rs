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

    let log_level = if cli.verbose {
        "debug"
    } else if cli.quiet {
        "warn"
    } else {
        "info"
    };
    init_tracing(log_level)?;

    let config = match cli.config.as_deref() {
        Some(path) => load_config_from(path)?,
        None => Config::load().await?,
    };

    match cli.command {
        Commands::Init { force } => init_project(force).await?,

        Commands::Build {
            task,
            output,
            lang,
            test,
            no_security,
        } => {
            let request = BuildRequest {
                task: task.join(" "),
                output_dir: output,
                language: lang,
                run_tests: test,
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

            let result = orchestrator.execute(request).await?;
            println!("\n✅ Build completed using {}:", result.model_used);
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

            println!("\n🖥️  Hardware Profile:");
            println!("   OS: {}", health.os);
            println!("   RAM: {} MB total", health.total_ram_mb);
            println!(
                "   VRAM: {} MB ({} MB free)",
                health.total_vram_mb, health.free_vram_mb
            );
            println!("   CPU Cores: {}", health.cpu_cores);
            println!("   Recommended Model: {}", health.recommended_model);
            println!("   Optimal Context: {}", health.optimal_context);

            if models {
                println!("\n📦 Available Models:");
                match ollama.list_models().await {
                    Ok(model_list) => {
                        for model in model_list {
                            println!("   - {} ({})", model.name, model.size_human);
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "forge: could not list models from {}: {e}",
                            config.ollama_url
                        );
                        eprintln!("       is `ollama serve` running?");
                    }
                }
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
                    if let Some(skill) = engine.find_skill(&query).await {
                        println!("\n🔍 Found skill: {}", skill.name);
                        println!("   {}", skill.description);
                    } else {
                        println!("No skill found matching: {query}");
                    }
                }
            }
        }

        Commands::Preload { model, keep_alive } => {
            let ollama = OllamaProvider::new(&config.ollama_url);
            let model = model.unwrap_or_else(|| config.default_model.clone());
            let start = std::time::Instant::now();
            print!("warming `{model}` (keep_alive={keep_alive})… ");
            // Flush so the user sees progress before the blocking call.
            use std::io::Write;
            let _ = std::io::stdout().flush();
            match ollama.preload(&model, &keep_alive).await {
                Ok(()) => {
                    println!("ok ({} ms)", start.elapsed().as_millis());
                    println!();
                    println!("Next call to this model will skip the cold-start.");
                    println!("Verify with: ollama ps");
                }
                Err(e) => {
                    println!("failed");
                    eprintln!("forge: {e:#}");
                    eprintln!("       is `ollama serve` running at {}?", config.ollama_url);
                    eprintln!("       has `{model}` been pulled? try `ollama pull {model}`");
                    std::process::exit(1);
                }
            }
        }

        Commands::Audit { path, secrets: _ } => {
            if !path.exists() {
                anyhow::bail!("audit path does not exist: {}", path.display());
            }
            let guard = SecurityGuard::new(true);
            let report = guard.audit_directory(&path).await?;

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

        Commands::Analyze { .. } | Commands::Parallel { .. } | Commands::Test { .. } => {
            anyhow::bail!(
                "this subcommand is not implemented in v0.1.0 — see the status table in README.md"
            );
        }
    }

    Ok(())
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
