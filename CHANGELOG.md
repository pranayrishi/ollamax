# Changelog

All notable changes to Ollama-Forge are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This project does
**not** yet follow semantic versioning — every 0.x release may break things.

## [Unreleased]

### Added (session 2)
- **`forge audit <path>`** is now wired up: walks the directory, scans every
  scannable file with `SecurityGuard`, prints findings grouped by severity,
  and exits 1 when any Critical/High match is found (so it can be a
  pre-commit hook). Skips `target/`, `node_modules/`, `.git/`, `dist/`,
  `build/`, `vendor/`, `venv/`, `__pycache__/`, and dotdirs.
- **`forge preload <model>`**: warm-loads a model into Ollama with a
  configurable `--keep-alive`. Removes the cold-start tax on the next call.
- **Inline finding suppression** in the secret scanner. Add `// forge:allow`
  (or `# forge:allow`) on any line to silence findings on that line.
  Documented use case: regex pattern *definitions* in the scanner itself.
- **Real VRAM detection** in `monitoring/`:
  - `nvidia-smi --query-gpu=memory.total,memory.free` parsed across all GPUs.
  - `rocm-smi --showmeminfo vram --json` for AMD on Linux.
  - Apple Silicon: `sysctl hw.optional.arm64` + 70% of total RAM as the
    Metal-addressable budget (replaces a wrong "divide RAM by 4" heuristic).
  - Intel Mac: `system_profiler SPDisplaysDataType -json`.
  - **CPU-only is now reported honestly** as `(GpuKind::Cpu, 0, 0)` instead
    of fabricating `(16384, 8192)`.
- New `GpuKind` enum (`Nvidia`/`Amd`/`AppleSilicon`/`AppleIntel`/`Cpu`) so the
  CLI can explain *why* it picked a model.
- **Keep_alive discipline in the parallel executor**: every `execute_parallel`
  call preloads the routed model with `keep_alive=1h` *before* fanning out.
  Workers re-use the warm model and pass `keep_alive` themselves so the
  residency window survives the whole batch.
- Subtask workers now use the **routed model + tier-correct `num_ctx`**
  instead of the previous hardcoded `llama3.2:3b` / `8192`.
- `Commands::Analyze`/`Parallel`/`Test` now `bail!` with a clear "not
  implemented in v0.1.0" message instead of falling through a `_` arm to
  "use forge --help" — closes the gap between advertised and real surface.
- **`tests/monitoring_logic.rs`** (6 tests): pin model-tier ladder boundaries
  so tweaking `suggest_model` requires updating the test on purpose.
- **`tests/security_scanner.rs`** (10 tests): cover detection, suppression,
  command validation, dir-walking exclusions, and disabled-state.
- **`tests/router_complexity.rs`** (6 tests): pin classifier behavior; one
  test caught a real fallback bug (see Fixed below).
- **`tests/context_manager.rs`** (5 tests): sliding-window eviction,
  truncation, system-prompt ordering.
- **`.github/workflows/ci.yml`**: ubuntu + macos matrix, runs `cargo fmt
  --check`, `cargo clippy -D warnings`, `cargo test`, and `./install.sh
  --dry-run` on every push and PR.

### Fixed (session 2)
- `SecurityGuard::scan_content` did not honor the `enabled` flag — only
  `scan_file` and `audit_directory` did. Caught by a test.
- The fork-bomb detection regex was `:\(\)\{.*\};.*\$`, which required a
  literal `$` at the end of the line. The actual fork bomb (`:(){ :|:& };:`)
  doesn't contain `$`, so the rule never fired. Replaced with a pattern that
  matches the function-definition shape. Caught by a test.
- `TaskRouter::route_to_model` could return a hardcoded default model that
  was not in the user's `available_models` list, causing downstream Ollama
  calls to 404. Now walks *available* models in size order based on the
  task's complexity tier. Caught by a test.
- `Cargo.toml`: removed 11 unused dependencies (`tracing-appender`, `infer`,
  `tokio-util`, `async-stream`, `criterion`, `proptest`, `mockall`,
  `wiremock`, `parking_lot`, `once_cell`, `futures`) and the `tracing`
  feature on `tokio`, the `json` feature on `tracing-subscriber`, and
  `indicatif`. Faster cold builds, smaller dep tree.
- Cleared 25 of 31 dead-code/unused-import warnings via `cargo fix`. The
  remaining 6 are now `#[allow(dead_code)]` with explanatory comments
  (scaffolded fields like `tdd`, `router`, `workers` that exist for the
  future-wiring contract).

### Changed (session 2)
- `install.sh` already exists — no changes needed this session.
- Model tier ladder in `suggest_model` is now `qwen2.5-coder` family across
  most tiers (the strongest open coding-tuned model line at 1.5B/3B/7B/14B/32B),
  not a mix of `codellama`/`phi4`/`qwen`. One coherent ladder.


### Fixed
- `src/context/mod.rs`: malformed raw string in `Modelfile` SYSTEM block was
  blocking the entire crate from compiling (`E0765`).
- `src/security/mod.rs`: two regex patterns used `\"` inside `r"…"` raw
  strings, which is a syntax error (`E0762`). Switched to `r#"…"#`.
- `src/security/mod.rs`: `scan_file` was iterating over a `Future` instead of
  the `Vec<SecurityFinding>` it returns; added the missing `.await`. Also
  fixed access to `finding.severity` / `finding.description`, which actually
  live on `finding.rule`.
- `src/orchestrator/mod.rs`: `findings` was moved into the struct literal
  before being borrowed for `len()`, blocking compile.
- `src/providers/mod.rs`: `ProviderPool` had `#[derive(Debug)]` but its
  `Arc<dyn LlmProvider>` field is not `Debug`. Replaced with a manual impl.
- `src/providers/mod.rs`: `register()` moved `name` then logged it,
  use-after-move. Reordered.
- `src/providers/ollama.rs`: `ChatMessageDto` was used as a deserialize target
  but only derived `Serialize`.
- `src/cli/mod.rs`: referenced a nonexistent `forge_lib` crate. Repointed to
  `crate::orchestrator::BuildRequest`.
- `src/main.rs`: `forge_lib` shim, sync `Config::load()` calling async
  `tokio::fs::read_to_string`, and a dead `_` arm. Rewritten cleanly to use
  `ollama_forge::*` paths.
- `install.sh`: missing `set -u`/`pipefail`, raw `${INSTALL_DIR}/bin` line
  appended to shell rc files instead of an `export PATH=…` statement (broke
  every user's shell), no idempotency check, no preflight for `cargo` or
  `ollama`. Rewritten with `--dry-run`, `--update-shell`, `--prefix`, and
  preflight checks.

### Added
- `tests/skill_recipes_parse.rs`: smoke test that every JSON in
  `skills/recipes/` deserializes into a `Skill`.
- `lib.rs`: `Config::load()` async constructor that returns
  `Config::default()` cleanly when the config file is absent.
- `SECURITY.md` with a stated threat model.
- This `CHANGELOG.md`.

### Changed
- `README.md`: removed fabricated benchmarks, fake "200-2000ms vs <50ms"
  comparison vs Claude Code, fake star-count badge pointing at a repository
  that does not exist, and broken `docs/*.md` links. Replaced with a status
  table showing what actually works in v0.1.0 (`status`, `optimize`,
  `skills list`) vs what is scaffolded (`build`, `parallel`, `skills add`).

## [0.1.0] - initial scaffold

- Module layout for `cli`, `orchestrator`, `router`, `executor`, `context`,
  `providers`, `security`, `monitoring`, `skills`.
- Bundled skill recipes: `docker-expert`, `security-auditor`,
  `react-native-expert`, `api-designer`.
- Did not compile.
