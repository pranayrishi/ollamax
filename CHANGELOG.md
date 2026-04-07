# Changelog

All notable changes to Ollama-Forge are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This project does
**not** yet follow semantic versioning — every 0.x release may break things.

## [Unreleased]

### Added (session 6)

#### Persistent always-rules

- **`src/rules/`**: a `RuleSet` loaded from
  `~/.config/ollama-forge/rules/*.md` (or `$XDG_CONFIG_HOME` equivalent).
  Two file flavors: plain Markdown (whole file = one rule) and
  YAML-frontmatter (`name`/`description`/`tags` + Markdown body, same
  shape as `SKILL.md`). Files are sorted alphabetically so users control
  ordering with `00-`, `10-`, `20-` prefixes.
- Rules are now **automatically prepended to every system prompt** across:
  `forge chat`, `forge research` (via `AgentConfig::system_suffix`),
  `forge run-skill`, `forge analyze` (review pass), `forge test`, and
  every worker in the `forge build` orchestrator (via
  `OrchestratorConfig::rules_suffix`). One source of truth, no per-command
  flag.
- **`forge rules list/init/show/path`** subcommands. `init` writes a
  starter rule file. `show` prints the rendered concatenation that gets
  injected. `path` is grep-friendly for shell scripts.
- **Validated end-to-end against real Ollama**: created a starter rule
  saying "use 4-space indentation in Rust", asked llama3.2 the indentation
  question, and the model answered correctly using the rule.

#### Continuous-learning loop

- **`src/instincts/`**: read-only analyzer over the replay log. Surfaces
  repeated tasks (same prompt 3+ times) and repeated system prompts as
  candidate skills/rules. **Intentionally does not auto-promote** — the
  replay log contains the user's full prompt history including private
  code, and auto-extracting that into a shared skill is a privacy
  footgun. Human-in-the-loop is the safer default.
- **`forge instincts [<log>] [--threshold N]`** command. Defaults to the
  3-occurrence floor; `--threshold` lowers it for users with small logs.
  Prints next-step instructions: "to promote → write a skill JSON / drop
  a rule .md".
- **Validated end-to-end**: ran `forge chat` three times with the same
  prompt under `FORGE_REPLAY_LOG`, then `forge instincts` correctly
  surfaced the pattern with `count=3`.

#### Politeness layer

- **robots.txt support in `fetch_url`**. Per-host cache,
  RFC-9309-style group resolution (targeted user-agent group wins
  outright over the wildcard, doesn't merge with it), opt-out via
  `FORGE_IGNORE_ROBOTS=1` for cases where the user is hitting their own
  staging server.
- **`FORGE_TRACE_WIDTH`**: configurable preview width for
  `forge research --trace`. Bumped default from 100 to 300 chars so URLs
  and citations no longer get cut.

### Fixed (session 6)

- **CI was failing on Linux** with `parse_vram_string is never used` because
  the function is only called from `detect_macos_intel_vram` (which is
  itself `cfg(target_os = "macos")`) but wasn't gated the same way. On
  Linux clippy with `-D warnings`, that's a hard error. Gated to macOS.
- **Node.js 20 deprecation warnings** from `actions/checkout@v4` and
  `actions/cache@v4`: set `FORCE_JAVASCRIPT_ACTIONS_TO_NODE24=true` on
  the workflow so the runner forces Node 24.
- **The `forge build` smoke test exposed two real bugs:**
  1. **Orchestrator handed `complexity.suggested_model` straight to the
     executor without checking it's installed.** When the analyzer's pick
     wasn't pulled, Ollama hung trying to fetch it (5-minute timeout, no
     useful error). Now the orchestrator calls `route_to_model` (which is
     guaranteed to return an available model) before dispatching.
  2. **The labeled-code-block extractor only looked at the fence line for
     the path.** Small models (`qwen3-vl:2b`) put the path on the first
     line *inside* the block instead of on the fence: `\`\`\`rust\nsrc/lib.rs\n…`.
     Added a `looks_like_path` heuristic + peek-fallback so both shapes
     work. Pinned by 4 new tests in `tests/build_extractor.rs`.
- **`OllamaProvider::preload` now has a per-call 120s timeout**, not the
  client-wide 300s default. A 70B cold-load can legitimately take 60-90s,
  but "the model isn't installed and Ollama hangs trying to pull it" used
  to lock the whole process for five minutes with no useful error.
- **`BuildResult` always returned `tokens_generated: 0` and `duration_ms: 0`.**
  These are part of the public API. Now sums across worker results, and
  `forge build` prints them on completion. Failed worker errors are
  surfaced via `BuildResult::warnings`.
- **robots.txt parser was over-blocking.** When both a `User-agent: *`
  and a `User-agent: ollama-forge` group existed, the previous flat
  parser put both groups' rules into the same disallow list, so wildcard
  rules like `Disallow: /everything` would block our agent even when the
  targeted group only had `Disallow: /api`. Rewrote with proper
  per-group tracking; targeted group now wins outright per RFC 9309.
  Pinned by 3 new tests.

### Added (session 5)

#### Limitation 1 — Tool-using research agent (free-only)

- **`src/tools/`**: new module with a `Tool` trait, `ToolRegistry`, and four
  bundled tools. **Every tool hits a free, no-API-key endpoint:**
  - `web_search` → DuckDuckGo Instant Answer JSON API
  - `wikipedia` → Wikipedia REST `summary` + opensearch
  - `arxiv` → arXiv Atom API (hand-rolled minimal parser, no XML dep)
  - `fetch_url` → plain HTTP GET with a tag-stripping HTML→text pass
- **`src/agent/`**: tool-calling loop. Uses Ollama's `format: "json"`
  parameter so the model is forced to emit parseable JSON, then dispatches
  either to a tool or to a final answer. Few-shot examples in the system
  prompt teach small (7-8B) models how to populate args. Capped at
  `max_iterations` rounds (default 6, configurable). Recovers from
  malformed-JSON responses by retrying with a hint instead of crashing.
- **`forge research "<question>"`**: end-to-end command. Picks an installed
  model with size-based fallback, prints a per-step trace to stderr,
  streams the final answer to stdout. **Validated end-to-end against real
  Ollama on this machine** — Llama 3.1 8B successfully called arXiv twice
  with proper args and synthesized an answer.
- **`forge tools`**: lists the four bundled tools and the JSON schema the
  agent uses to call them.
- **Per-tool rate limit (250 ms)** in `ToolRegistry::get_rate_limited` so
  the agent loop can't hammer DDG/Wikipedia/arXiv into a temporary IP ban.

#### Limitation 2 — Heterogeneous parallel execution

- **`SubTask::model_override` and `num_ctx_override`**: each subtask in a
  parallel build can now declare a different model. Closes the previous
  "every worker uses the same model" limitation.
- **`TaskRouter::split_into_tiered_subtasks`**: assigns architecture work
  to the largest installed model, boilerplate (frontend/tests) to the
  smallest, balanced work to the analyzer's pick. Inserts an explicit
  Architecture subtask at position 0 if the user's task doesn't already
  have one — gives the big model something useful to do that's *different*
  from what the small model is doing in parallel.
- **`TaskRouter::split_into_tiered_subtasks_vram_aware`**: VRAM-aware
  variant. If the sum of selected models wouldn't fit in `free_vram_mb`
  (with a 30% safety margin for KV cache), collapses every override to
  the largest single model that does fit. Prevents silent OOM kills.
- **`ParallelExecutor::execute_parallel_with_progress`**: collects every
  unique model used in the batch and **preloads them concurrently**
  (Ollama serializes per-model, not across models, so a 32B and a 3B can
  warm up at the same time). Each worker then dispatches with its
  per-subtask model + num_ctx.
- **`ProgressEvent` channel**: every preload and worker emits start/finish
  events on a `tokio::mpsc::UnboundedSender` so the CLI can stream a live
  status board to stderr during long parallel builds.

#### Compliance / replay

- **`src/replay/`**: append-only JSONL log of every Ollama call. Records
  the model name, **`/api/tags` digest** (so a `ollama pull` of the same
  tag is detectable), seed, temperature, top_p, num_ctx, format, system
  prompt, prompt, **real SHA-256** of prompt and response, and the
  forge version string including the git SHA.
- **`forge replay <log>`**: re-issues every Ollama call in a JSONL log
  against locally-installed models and reports hash drift. Exits 1 on
  any drift, so this can wedge into CI as a "the model rotated under us"
  detector.
- **`GenerateOptions::seed`**: PRNG seed forwarded to Ollama
  (`options.seed`). Combined with `temperature: 0` this gives
  bit-identical output on the same model — the foundation of replay.
  Validated end-to-end: a `chat` call written to `FORGE_REPLAY_LOG=` and
  then replayed produces a byte-identical SHA-256.
- **`FORGE_REPLAY_LOG=path` env var**: opt-in replay logging. Wired into
  `forge chat` and `forge research` (the agent loop logs every iteration).
  When set, `forge chat` automatically switches to `seed=0`/`temp=0` so
  the resulting log is replayable instead of unrepeatable.

#### `forge build --output`

- **Labeled-code-block extractor**: ` ```rust src/main.rs ` style fenced
  blocks are extracted and written to disk under `--output dir/`. Tolerates
  `// `, `# `, and `file=` prefixes on the path. Rejects `..` and
  absolute paths to prevent escape from the output dir. Pinned by 6 unit
  tests in `tests/build_extractor.rs`.

### Fixed (session 5)

- **`replay::quick_hash` was using `std::hash::DefaultHasher`** which is
  documented as "may change between Rust releases" and uses SipHash-1-3.
  Replay logs would have silently drifted on a future stdlib bump,
  invalidating the entire compliance pitch. Replaced with real SHA-256
  via `sha2 = "0.10"`. The test pins the SHA-256 of `"hello"` so any
  future bump that breaks the hash function trips immediately.
- **`agent::run` propagated `serde_json::from_str` errors via `?`** which
  killed the whole research session on a single malformed model output.
  Small models hiccup ~5% of the time even with `format: "json"`. Added
  `extract_first_json_object` recovery (handles `Sure, here you go: {...}`
  prefixes via a brace-counting parser that skips over quoted strings),
  and on total failure appends an "your last response was malformed"
  hint to the transcript and retries instead of crashing.
- **Replay records had `model_digest: ""`** because we were trying to
  read it from `/api/show` which doesn't expose the field on this Ollama
  version. Switched to `/api/tags` (the list endpoint) which has been
  stable since v0.1.x. **Validated against real Ollama** — digest now
  populates correctly.
- **Replay logging only ran from `forge chat`**. The agent loop, where
  deterministic replay is most valuable, bypassed it. Wired
  `FORGE_REPLAY_LOG` directly into `Agent::run` with a cached digest
  (one `/api/tags` call per session, not per iteration).
- **Heterogeneous routing assigned models without checking combined
  VRAM**. On an 8 GB card with a 7B and a 1.5B installed, both would be
  scheduled and the second load would OOM. Added the VRAM-aware variant.
- **`MAX_ITERATIONS = 6` was hardcoded**. Deep research questions need
  8-10 rounds. Now configurable via `AgentConfig::max_iterations` and the
  `--max-iterations` CLI flag.
- **`fetch_url` called `Response::bytes()`** which buffers the entire
  body before truncation. A 1 GB blob would have OOM'd the process before
  hitting the 32 KB cap. Now streams chunks via `Response::chunk()` and
  breaks at the cap. Also rejects up-front if `Content-Length` declares
  more than 4× the cap.

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
