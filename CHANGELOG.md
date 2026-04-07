# Changelog

All notable changes to Ollama-Forge are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This project does
**not** yet follow semantic versioning — every 0.x release may break things.

## [Unreleased]

### Added (session 4)
- **`forge run-skill <name> "<task>"`**: load a skill, pick the right
  installed model (with size-based fallback if the recommended one isn't
  pulled), pass the skill's system prompt + planning + execution guidance,
  stream the model's tokens to stdout. Closes the loop between the skills
  engine and the user surface.
- **`forge analyze <dir>`**: combines the local secret scanner with a
  model-driven code review pass. Token-budgeted by `tiktoken-rs` so a 50 KB
  src/ doesn't blow out a 16k context — reserves ~30% headroom for the
  response.
- **`forge test <file>`**: generates a complete test file for the target,
  picks the framework based on the source extension (Rust → `#[test]` /
  `#[tokio::test]`, Python → pytest, TS → Vitest, Go → standard testing,
  etc.), streams to stdout so the user can pipe to `> tests/foo_test.rs`.
- **`forge audit --json`**: emits a stable JSON shape (`schema_version: 1`,
  `forge_version`, findings array with severity/file/line/rule) for CI
  consumers and pre-commit hooks. `jq`-friendly.
- **`forge preload`** now ships a braille spinner so a 14 GB cold-load of a
  big model doesn't look like a hang. Suppress with `FORGE_NO_SPINNER=1`.
- **`forge status`** now probes Ollama via `/api/tags` and `/api/ps`, prints
  whether the daemon is reachable, and lists currently-loaded models with
  their VRAM footprint and `keep_alive` expiry. Single most useful line in
  the entire CLI.
- **`forge --version`** now includes the git short SHA (via `build.rs`), so
  a build can be pinned for replay/debug purposes — the foundation for the
  deterministic-replay log on the roadmap.
- **Real BPE token counting** via `tiktoken-rs` (cl100k_base). Replaces the
  previous `chars/3` estimator. Both `ContextManager` and `forge analyze`
  use it. ~10% accurate vs Llama/Qwen tokenizers, far better than the
  whitespace counter from session 1.
- **Schema-constrained output**: `GenerateOptions::format` carries an
  Ollama `format` parameter (v0.5+) — either `"json"` for free-form valid
  JSON or a full JSON Schema for constrained decoding. Wired through
  `OllamaProvider::generate_streaming` and `generate`. Live integration
  test in `tests/structured_output.rs` (gated by `FORGE_LIVE_OLLAMA=1`).
  This is the local-LLM equivalent of OpenAI's `response_format` and the
  closest thing forge has to "guaranteed-valid tool calls" without dropping
  into raw GBNF grammars.
- **SKILL.md (YAML-frontmatter) compatibility**: drop a
  `<name>/SKILL.md` (Anthropic format) into your skills dir and forge loads
  it alongside the JSON recipes. Markdown body becomes the system prompt
  verbatim. 5 unit tests pin the parser contract.
- **`OllamaProvider::running_models`**: queries `/api/ps` and returns a
  list of currently-loaded models, their VRAM footprint, and expiry.
- **`OllamaProvider::try_new`**: fallible constructor for libraries that
  don't want to take down the process on a TLS-init failure.
- **`tests/structured_output.rs`**: 1 live test for schema-constrained
  output (gated).
- **`tests/skill_md_compat.rs`**: 5 tests for SKILL.md frontmatter parser.
- **`build.rs`**: stamps the binary with `FORGE_GIT_SHA` so `--version`
  can include it. Falls back to `unknown` outside a git checkout.
- **`.github/workflows/release.yml`**: tag-triggered (`v*`) release workflow
  that builds for `x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`, and
  `x86_64-apple-darwin` on native runners (no cross-compilation), strips +
  smoke-tests + tarballs each binary, and attaches everything (with
  SHA-256 sums) to a GitHub release.
- **`install.sh --prebuilt`**: tries to fetch a release tarball for the
  detected target triple before falling back to a source build. Closes the
  loop between the release workflow and the installer.

### Changed (session 4)
- **Default tracing log level is now `warn`**, not `info`. Previously every
  user command spammed `INFO ollama_forge::*` lines on stderr — including
  `--json` consumers. `--verbose` brings the chatty output back when needed.
- **Section-aware merger** in `ParallelExecutor::merge_results`: each
  worker's output is wrapped in explicit `// === BEGIN/END section ===`
  markers and the merger is told they're load-bearing. Replaces the previous
  "here are some snippets, combine them" prompt that produced hallucinated
  stitching and dropped sections. Temperature dropped to 0.1 (merging is
  not creative work). The merger uses the same model the workers used to
  avoid an extra cold-start mid-build.
- **`forge build` CLI surface trimmed**: `--output`, `--lang`, and `--test`
  flags removed. `--output` was captured into `BuildRequest` and never
  written anywhere. `--lang` was passed through and never read. `--test`
  was a boolean that did nothing.
- **`SecurityGuard::audit_directory`** skips `.git/`, `target/`,
  `node_modules/`, `dist/`, `build/`, `vendor/`, `venv/`, `__pycache__/`,
  `.cargo/`, and any dotdir. (Already done in session 2; reaffirmed.)
- **Three-way default-model alignment**: `Config::default`,
  `OrchestratorConfig::default`, and `STARTER_FORGE_TOML` now agree on
  `qwen2.5-coder:7b`. Previously the three places said
  `llama3.2:3b`/`qwen2.5-coder:7b`/`qwen2.5-coder:7b` independently.
- **`forge skills search`** now lists *all* matching skills, not just the
  first one. Vague queries like `react` will surface every relevant skill.
- **`tests/skill_recipes_parse.rs`**: now also asserts `prompts.system` is
  non-empty and `tags` is non-empty. A skill with an empty system prompt is
  a no-op when `forge run-skill` is invoked; a skill with no tags is
  invisible to `forge skills search`.
- **README status table**: refreshed to show every command implemented in
  v0.1.0, including `run-skill`, `analyze`, `test`, `audit --json`, the
  spinner on `preload`, and the new `forge --version` git-SHA stamping.

### Fixed (session 4)
- **`ContextManager::token_counts`** was a `HashMap<id, tokens>` written by
  `add()` and cleared by `clear()` but never read by anything — `stats()`
  iterates `history` directly. It also didn't track evictions from the
  sliding window, so on a real workload it would have leaked memory and
  reported incorrect counts to anyone who decided to read it. Removed the
  field, the `HashMap` import, and both write paths. Lockless win for the
  hot path of `ContextManager::add`.

### Added (session 3)
- **Real streaming for `forge chat`**: new `OllamaProvider::generate_streaming`
  uses `Response::chunk()` to drain Ollama's NDJSON line-by-line and emits
  text via a callback. `forge chat` now prints tokens as they arrive instead
  of buffering the entire response and dumping it after a 20-second wait.
- **`forge skills add <path>`** is now wired: reads a JSON skill file from a
  local path, validates with serde, and persists via `SkillsEngine::add_skill`.
  Remote URL fetching is intentionally not supported in v0.1.0 — adding it
  would punch a hole through the "no network calls but ollama" property.
- `OllamaProvider::try_new` — fallible constructor for callers that don't
  want a panic on TLS-init failure.

### Fixed (session 3)
- **`SkillsEngine` first-run was writing stripped-down hardcoded skills** that
  had drifted out of sync with the JSON files in `skills/recipes/`. The
  bundled `docker-expert.json` ships 2 recipes with 4 steps total; the
  hardcoded version had 1 recipe with 1 step. Now the actual JSONs are baked
  into the binary via `include_str!` and there's a single source of truth.
- **`ContextManager::count_tokens` was `text.split_whitespace().count()`**,
  which on a 50 KB code file reports ~7k tokens when the real count is ~17k.
  Replaced with a `chars / 3` estimator calibrated against tiktoken
  cl100k_base on a code/prose mix. Errs toward over-counting so the
  sliding-window evictor fires before Ollama silently truncates. Pure
  function, fully unit-tested. Real tokenizer integration tracked for later.
- **`Cli::output: Option<OutputFormat>` was a global flag that was never
  read** anywhere — the `--output json` advertised by `--help` did nothing.
  Removed both the flag and the `OutputFormat` enum.
- **`lib::init()` was a dead-code duplicate of `Config::load()`** added in
  session 1, with subtly different behavior (sync vs async, missing
  `await`). Removed.
- **`init_project()` was using `include_str!("../forge.toml")`** which baked
  whatever was in the developer's local `forge.toml` (including any of their
  edits) into every release binary. Replaced with a `STARTER_FORGE_TOML`
  const that won't drift.
- **`Cargo.toml`'s `repository`/`authors`/`homepage`** pointed at a
  nonexistent `ollama-forge/ollama-forge` org and an `ollama-forge.ai` domain
  that does not exist. Repointed to `pranayrishi/ollamax` and dropped the
  fake homepage. Author set to the actual maintainer.
- **`forge init` next-steps text** was `forge "build a chat app"` —
  out-of-date and references the unimplemented build path. Replaced with
  `forge status`/`preload`/`audit`/`chat` — the four commands that actually
  work in v0.1.0.

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
