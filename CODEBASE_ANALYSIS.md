# Ollama-Forge — Codebase Analysis & Re-Orientation Guide

> Generated 2026-06-17 as a read-only catch-up document. No code was modified.
> Everything below is derived from reading the source; where I'm inferring
> rather than certain, I say so.

---

## 1. Executive Summary

**Ollama-Forge** (package `ollama-forge`, binary `forge`) is a single-binary
Rust CLI that acts as a **harness / optimization layer for local AI coding
agents that run on [Ollama](https://ollama.com)**. The pitch: you want AI
coding help *without* shipping your code to a cloud provider, so everything
runs against a local Ollama daemon at `http://localhost:11434`. Forge adds the
"glue" the local-first ecosystem is missing — it detects your hardware and
picks a model that actually fits your VRAM, keeps models warm to avoid
cold-starts, runs a tool-using research agent on local models, scans files for
leaked secrets *before* they're sent to the model, and keeps a cryptographic,
**deterministic replay log** so an AI-assisted change can be reproduced
bit-for-bit later (the compliance angle for finance/healthcare/defense).

The single main goal in one line: **make local LLMs a viable, hardware-aware,
auditable coding assistant on your own machine, with zero paid API calls.**

Status is **pre-alpha v0.1.0**. Most commands work; a few headline features
(end-to-end orchestrator, TDD enforcement, LoRA training) are scaffolded or
deliberately out-of-scope. The git history shows the project was built across
seven numbered "sessions," each closing the previous session's loose ends —
see [CHANGELOG.md](CHANGELOG.md).

---

## 2. Tech Stack & Dependencies

**Language / runtime:** Rust (edition 2021, MSRV 1.75), async on **Tokio**.
Builds to one binary, `forge`. Config in [Cargo.toml](Cargo.toml).

**Build & release:** Cargo; release profile uses LTO + `codegen-units=1` +
`strip` + `panic=abort` for a small fast binary. A [build.rs](build.rs) stamps
the git short SHA into the version string. CI in
[.github/workflows/ci.yml](.github/workflows/ci.yml) (fmt + clippy
`-D warnings` + build + test on Ubuntu & macOS); release binaries built per
native target in [.github/workflows/release.yml](.github/workflows/release.yml).

Major dependencies (each with the reason it's present — most of these are
annotated directly in [Cargo.toml](Cargo.toml:21)):

| Crate | Why it's here |
| :-- | :-- |
| `tokio` (full) | Async runtime — every network call and the worker pool are async |
| `reqwest` (rustls-tls, no OpenSSL) | The *only* HTTP client; talks to Ollama and the free web tools |
| `serde` / `serde_json` | Ollama wire protocol, skill recipes, replay log, JSON tool args |
| `serde_yaml` | Config file (`config.yaml`) + SKILL.md / rules YAML frontmatter |
| `clap` (derive) | CLI parsing — all subcommands in [src/cli/mod.rs](src/cli/mod.rs) |
| `anyhow` / `thiserror` | Error handling |
| `tracing` / `tracing-subscriber` | Logging (level set by `--verbose`/`--quiet`) |
| `async-trait` | The `LlmProvider` and `Tool` trait abstractions |
| `sysinfo` | RAM / CPU detection in the hardware sentinel |
| `dirs` | Locating `~/.config/ollama-forge/...` |
| `chrono` | Timestamps (replay log, audit log, session) |
| `uuid` | Session IDs, subtask/worker IDs |
| `regex` | The secret scanner's pattern engine |
| `walkdir` | Recursive directory walks for `audit` / `analyze` / `finetune` |
| `tiktoken-rs` | **Real BPE token counting** (cl100k_base) for context budgeting |
| `sha2` | **Real SHA-256** for replay hashes (stable across Rust versions) |
| `tempfile` (dev) | Sandbox dirs in tests |

There is **no database, no message queue, no web server, no cloud SDK**. The
only external network endpoints are Ollama (localhost) and four free,
keyless public APIs used by the research agent (see §7).

---

## 3. Directory & File Map

```
Ollama-Optimizer/                 (repo root; crate name is "ollama-forge")
├── Cargo.toml / Cargo.lock       crate manifest + locked deps
├── build.rs                      stamps git SHA into FORGE_GIT_SHA at build time
├── install.sh                    from-source (or --prebuilt) installer → ~/.local/forge/bin
├── forge.toml                    ⚠️ sample TOML config — NOT actually parsed (see §8)
├── README.md (+ .zh/.ja/.de/.pt) docs; the README is the best feature reference
├── CHANGELOG.md                  session-by-session history (1→7); great for "what was I doing"
├── CONTRIBUTING.md / SECURITY.md / LICENSE (MIT)
├── .github/workflows/            ci.yml (test matrix) + release.yml (tagged binaries)
├── skills/recipes/*.json         5 bundled skill recipes (baked into the binary)
├── editor-integrations/
│   └── forge.nvim/               thin Neovim plugin that shells out to `forge`
├── target/                       ⚙️ BUILD OUTPUT — generated, gitignored, ignore it
└── src/
    ├── main.rs                   ⭐ the CLI dispatch + most command bodies (1670 lines)
    ├── lib.rs                    module roots + Config struct + init_tracing
    ├── cli/mod.rs                clap Cli/Commands/subcommand enums + VERSION const
    ├── providers/
    │   ├── mod.rs                LlmProvider trait, GenerateOptions, ProviderPool
    │   └── ollama.rs             ⭐ the only real provider: Ollama HTTP client
    ├── agent/mod.rs              ⭐ tool-using research agent loop (JSON-action protocol)
    ├── tools/
    │   ├── mod.rs                Tool trait + ToolRegistry + rate limiter + truncation
    │   ├── web_search.rs         DuckDuckGo Instant Answer
    │   ├── wikipedia.rs          Wikipedia REST summary + opensearch
    │   ├── arxiv.rs              arXiv Atom API
    │   └── fetch_url.rs          plain HTTP GET + HTML→text + robots.txt
    ├── orchestrator/mod.rs       build pipeline: analyze → route → parallel → merge
    ├── router/mod.rs             complexity heuristic + heterogeneous model routing
    ├── executor/mod.rs           parallel worker pool + result merger
    ├── context/mod.rs            sliding-window context mgr + estimate_tokens + Modelfile gen
    ├── monitoring/mod.rs         VramSentinel: hardware detect + model/ctx/gpu-layer tiers
    ├── security/mod.rs           regex secret scanner + command validator + TddEnforcer
    ├── skills/mod.rs             SkillsEngine: JSON recipes + SKILL.md loader
    ├── replay/mod.rs             deterministic JSONL replay log (append/stream/hash)
    ├── rules/mod.rs              persistent user "always-rules" (~/.config/.../rules/*.md)
    └── instincts/mod.rs          read-only "continuous learning" over the replay log
```

**Flag as generated / vendored / not-load-bearing:**
- [target/](target/) — Cargo build output (5+ MB of fingerprints, the compiled
  binary, dependency artifacts). Entirely generated; gitignored.
- `Cargo.lock` — generated, but committed on purpose (it's a binary crate).
- [forge.toml](forge.toml) — looks load-bearing but **is not read by the
  config loader** (detailed in §8). Treat it as a sample/aspirational doc.
- Source line counts (real code): `main.rs` 1670, `router` 557, `ollama.rs`
  599, `fetch_url.rs` 538, `agent` 521, `executor` 515, `security` 484. Total
  ~8.7k lines of `src/` plus ~1k lines of integration tests.

---

## 4. Architecture & Data Flow

Forge is a **command dispatcher**: [src/main.rs](src/main.rs) builds the
Tokio runtime, parses the CLI, loads `Config` and the user "always-rules,"
then `match`es on the subcommand. There is no long-running process — each
invocation does its job and exits.

```
                         forge <subcommand> [args]
                                  │
                  main() → tokio runtime → async_main()  (src/main.rs:16)
                                  │
        ┌─────────────────────────┼───────────────────────────────┐
        │  Config::load()  (lib.rs)   RuleSet::load_default() (rules/) │
        └─────────────────────────┼───────────────────────────────┘
                                  │  rules_suffix appended to every system prompt
        ┌─────────────────────────┴───────────────────────────────┐
        ▼                ▼                ▼               ▼          ▼
   status/optimize   chat/run-skill   research        build      audit/analyze
   preload           test/finetune    (agent loop)  (orchestr.)   (security)
        │                │                │               │          │
   VramSentinel      OllamaProvider   Agent + Tools   Orchestrator  SecurityGuard
   (monitoring)      (providers)      (agent+tools)   (router+exec) (security)
        │                │                │               │          │
        └──────── all inference ─────────┴──── HTTP ──────┴──→  Ollama @ :11434
                                          └──── HTTP ──→ DDG / Wikipedia / arXiv / URLs
```

**Three principal data flows:**

1. **Single-shot generation** (`chat`, `run-skill`, `analyze`, `test`,
   `finetune`): build a `GenerateOptions`, call
   `OllamaProvider::generate_streaming`, stream tokens to stdout. If
   `FORGE_REPLAY_LOG` is set, append a replay record afterward.

2. **Research agent loop** (`research`): `Agent::run` (in
   [src/agent/mod.rs](src/agent/mod.rs:112)) drives an iterative
   JSON-action protocol. Each round it calls Ollama with `format="json"`,
   parses `{"action":"use_tool"|"answer", ...}`, dispatches to a `Tool` from
   the `ToolRegistry`, appends the result to the transcript, and loops up to
   `max_iterations` (default 6). Tools hit free public APIs.

3. **Parallel build** (`build`): `Orchestrator::execute_with_progress`
   ([src/orchestrator/mod.rs](src/orchestrator/mod.rs:131)) →
   `TaskRouter::analyze_complexity` (heuristic score) →
   `split_into_tiered_subtasks_vram_aware` (assign each subtask a model by
   role + VRAM budget) → `ParallelExecutor::execute_parallel_with_progress`
   (preload distinct models concurrently, run one worker per subtask) →
   `merge_results` (concatenate with section markers, then one
   low-temperature model pass to dedup). `main.rs` can then extract labeled
   code blocks to disk via `extract_and_write_code_blocks`.

The common thread: **`OllamaProvider` is the single choke point for all
inference**, and `VramSentinel` is the single source of truth for "what model
+ context fits this machine."

---

## 5. Key Components Deep-Dive

### `main.rs` — CLI bodies ([src/main.rs](src/main.rs))
Not just dispatch; most command logic lives here. Notables: streaming chat
(`generate_streaming` with a per-token closure), `forge build` progress
rendering over an mpsc channel, the braille `spinner_task`, `maybe_log_replay`
(records a replay entry after chat), and `extract_and_write_code_blocks`
([src/main.rs:1254](src/main.rs#L1254)) — the labeled-fenced-code-block
extractor with path-traversal guards (rejects `..` and absolute paths) and a
fallback that reads the path from the first line *inside* the block (small
models put it there).

### `providers/` — the Ollama client
[src/providers/mod.rs](src/providers/mod.rs) defines the async `LlmProvider`
trait, the big `GenerateOptions` struct (model, prompt, system, sampler knobs,
`seed`, `keep_alive`, and `format` for JSON/JSON-Schema constrained decoding),
and a `ProviderPool` (registry of named providers — currently only ever holds
Ollama). [src/providers/ollama.rs](src/providers/ollama.rs) is the real work:
`/api/generate` (buffered + true streaming NDJSON drain), `/api/chat`,
`/api/tags` (list models), `/api/ps` (running models), `preload` (warm-load
with `keep_alive`), `model_digest` (for replay), `health_check`. This is the
**only place `reqwest` is constructed for inference** — the README's privacy
claim hinges on that.

### `agent/` + `tools/` — the research agent
[src/agent/mod.rs](src/agent/mod.rs) implements the loop described in §4. Key
robustness tricks: it uses `format:"json"` (not a strict schema, because a
strict schema lets small models satisfy `args:{}`), few-shot examples in the
system prompt, a `extract_first_json_object` recovery parser for models that
wrap JSON in prose, a "this is your final turn" nudge on the last iteration,
and replay-logging of every agent call. `build_response_schema` exists but is
`#[allow(dead_code)]` — reserved for a future strict-schema mode.

[src/tools/mod.rs](src/tools/mod.rs) defines the `Tool` trait, the
`ToolRegistry` (with a 250 ms per-tool rate limiter via
`get_rate_limited`), and `truncate_for_model` (8 KB output budget). Four
bundled tools, all free/keyless:
- [web_search.rs](src/tools/web_search.rs) — DuckDuckGo Instant Answer
- [wikipedia.rs](src/tools/wikipedia.rs) — Wikipedia REST summary + opensearch
- [arxiv.rs](src/tools/arxiv.rs) — arXiv Atom API (with an Atom parser)
- [fetch_url.rs](src/tools/fetch_url.rs) — HTTP GET with a hand-rolled
  HTML→text stripper, **streaming byte cap** (won't OOM on a huge page), and a
  best-effort RFC-9309 `robots.txt` parser (bypass with `FORGE_IGNORE_ROBOTS=1`).

### `orchestrator/` + `router/` + `executor/` — the build pipeline
[orchestrator/mod.rs](src/orchestrator/mod.rs) wires everything for `forge
build`. [router/mod.rs](src/router/mod.rs) scores task complexity from keyword
buckets + length, classifies into `Simple/Medium/Complex/Architect`, and — the
interesting part — does **heterogeneous routing**: architecture work →
biggest installed model, boilerplate/tests/UI → smallest, balanced → the
analyzer's pick, all **VRAM-aware** (collapses to one model if the sum won't
fit; resident size estimated as disk×1.3). [executor/mod.rs](src/executor/mod.rs)
preloads the distinct models *concurrently*, spawns one Tokio task per
subtask, emits `ProgressEvent`s, and merges results (single success → verbatim;
multiple → section-marked concat + a temp-0.1 dedup pass). `MergingAgent`
(bottom of executor) is an alternate, hardcoded-model merger that appears
**unused** by the live path.

### `context/` — budgeting
[context/mod.rs](src/context/mod.rs): a sliding-window `ContextManager`
(evicts oldest entries past `max_tokens`) and the widely-used
`estimate_tokens` (real BPE via tiktoken cl100k_base, ~10% off Llama/Qwen but
far better than chars/3; falls back to chars/3). Also a `ModelfileGenerator`
that emits Ollama `Modelfile` text — largely superseded by `monitoring`.

### `monitoring/` — the hardware sentinel
[monitoring/mod.rs](src/monitoring/mod.rs): `VramSentinel::detect_hardware`
returns a `HardwareProfile`. VRAM detection is platform-specific and
best-effort: `nvidia-smi`, then `rocm-smi --json` (AMD), then Apple Silicon
(70% of unified RAM), then Intel Mac (`system_profiler`), else CPU `(0,0)`.
The pure tier functions `suggest_model` / `calculate_optimal_context` /
`calculate_gpu_layers` map free VRAM → a `qwen2.5-coder` ladder and are
unit-tested so the user-visible defaults can't drift silently.

### `security/` — the scanner
[security/mod.rs](src/security/mod.rs): `SecurityGuard` holds 8 regex rules
(AWS keys, private keys, API keys, DB URLs, JWTs, GitHub tokens, hardcoded
passwords, dangerous shell commands) with severities. `scan_content` honors a
`forge:allow` inline suppression. `audit_directory` walks a tree skipping
build/VCS/vendor dirs. `validate_command` checks for `rm -rf /`, fork bombs,
etc. Also houses `TddEnforcer` — **constructed but not wired into the build
path** (see §8).

### `skills/` — recipes + SKILL.md
[skills/mod.rs](src/skills/mod.rs): `SkillsEngine` loads `*.json` recipes and
Anthropic-style `SKILL.md` (YAML-frontmatter) files from
`~/.config/ollama-forge/skills/`. The 5 bundled recipes are baked into the
binary via `include_str!` and **synced to disk additively** on every run (new
recipes appear for existing users without clobbering edits). `match_skill_to_task`
matches by trigger keyword/tag.

### `replay/` + `instincts/` + `rules/` — the audit/learning trio
- [replay/mod.rs](src/replay/mod.rs): append-only JSONL log of every Ollama
  call (model digest, seed, sampler, prompt, SHA-256 of prompt+response).
  `forge replay` re-issues calls and reports hash drift. `stream_log` reads
  line-by-line so a big log doesn't blow memory.
- [instincts/mod.rs](src/instincts/mod.rs): **read-only** analysis of the
  replay log — surfaces repeated tasks, repeated system prompts, and repeated
  agent tool-chains (3+ occurrences) as *candidate* skills/rules. Deliberately
  never auto-promotes (privacy + small-model hallucination).
- [rules/mod.rs](src/rules/mod.rs): loads `~/.config/ollama-forge/rules/*.md`
  (plain or YAML-frontmatter), concatenates alphabetically, and the result
  (`rules_suffix`) is appended to **every** system prompt across commands.

---

## 6. Entry Points & How to Run It

**Prereqs:** [Ollama](https://ollama.com/download) installed with `ollama
serve` running, plus at least one pulled model (`ollama pull qwen2.5-coder:7b`).
Rust 1.75+ to build from source.

**Build / install / test:**
```bash
cargo build --release            # produces target/release/forge
./install.sh                     # builds + installs to ~/.local/forge/bin
./install.sh --prebuilt          # try a release tarball, fall back to source
cargo test                       # ~110 test fns; one live test is gated (see below)
cargo fmt --all -- --check       # CI gate
cargo clippy --all-targets -- -D warnings   # CI gate
```

**Most useful commands (all real in v0.1.0):**
```bash
forge status [--models]          # hardware + recommended model + Ollama health
forge optimize [--dry-run]       # prints a tuned Modelfile for your hardware
forge preload [model] [-k 1h]    # warm-load a model (braille spinner)
forge chat "..." [-m model]      # streaming chat
forge research "<q>" [--trace] [--max-iterations N]   # tool-using agent
forge tools                      # list the agent's tools + JSON schema
forge build "..." [-o ./out/]    # heterogeneous parallel build + extract code blocks
forge audit <dir> [--json]       # secret scan; exits 1 on Critical/High
forge analyze <dir> [-a full]    # secret scan + model code review
forge test <file> [-f framework] # generate a test file
forge skills list|add|search|remove ;  forge run-skill <name> "<task>"
forge rules init|list|show|path|edit [name]
forge finetune [<repo>] [-m model]    # bootstrap a local LoRA workflow (emits scripts)
forge replay <log.jsonl> [--verbose]  # re-run a session, report hash drift
forge instincts [<log>] [-t N]        # surface repeated patterns
forge init [--force]             # write a starter forge.toml (see caveat §8)
forge parallel                   # ❌ intentionally errors → use `forge build`
```

**Environment variables that matter:**
- `FORGE_REPLAY_LOG=path.jsonl` — enable replay logging (chat switches to
  seed=0/temp=0 for determinism).
- `FORGE_TRACE_WIDTH=N` — research trace preview width (default 300).
- `FORGE_IGNORE_ROBOTS=1` — let `fetch_url` ignore robots.txt.
- `FORGE_NO_SPINNER=1` — suppress the spinner (e.g., in logs).
- `FORGE_LIVE_OLLAMA=1` — run the one live integration test.
- `VISUAL` / `EDITOR` — used by `forge rules edit`.

**Config:** `Config::load()` reads
`~/.config/ollama-forge/config.yaml` (**YAML, flat keys** matching the
`Config` struct in [src/lib.rs](src/lib.rs:40)); missing file = defaults.
`--config <path>` also parses YAML. See the §8 caveat about `forge init`.

**Deploy:** push a `v*` git tag → [release.yml](.github/workflows/release.yml)
builds native binaries for Linux x86_64, Apple Silicon, and Intel Mac, and
attaches tarballs to a GitHub release. No release has been published yet.

---

## 7. External Integrations

All optional and all free; there is **no auth, no API keys, no telemetry**.

| Integration | Endpoint | Used by | Purpose |
| :-- | :-- | :-- | :-- |
| **Ollama** | `http://localhost:11434` (`/api/generate`, `/api/chat`, `/api/tags`, `/api/ps`) | every inference command | local LLM inference, model list, warm-load, digests |
| **DuckDuckGo** | `api.duckduckgo.com` Instant Answer JSON | `web_search` tool | general factual lookups |
| **Wikipedia** | `en.wikipedia.org` REST summary + opensearch | `wikipedia` tool | article lookup + fuzzy title search |
| **arXiv** | `export.arxiv.org/api/query` (Atom) | `arxiv` tool | academic paper search |
| **Arbitrary URLs** | any `http(s)` | `fetch_url` tool | read a page after discovering it (robots-aware) |
| `nvidia-smi` / `rocm-smi` / `system_profiler` / `sysctl` | local subprocess | `monitoring` | VRAM/GPU detection |
| `git` | local subprocess (build time) | `build.rs` | stamp version SHA |

No database, no object storage, no queue, no auth provider. State that
persists is just files under `~/.config/ollama-forge/` (skills, rules) and the
opt-in replay log wherever `FORGE_REPLAY_LOG` points.

---

## 8. State of the Project / Loose Ends

This is the section most useful for remembering where you left off. The code is
unusually honest about its own gaps (README "honest comparison" table, lots of
explanatory comments), and CI is green (fmt + clippy `-D warnings` + tests).

**⚠️ Config format mismatch — likely the biggest real bug.**
`forge init` writes a **TOML** file named `forge.toml` with sectioned keys
(`[forge]`, `[ollama]`, `[execution]`…) — see `STARTER_FORGE_TOML` at
[src/main.rs:1364](src/main.rs#L1364). But the loader `Config::load()` reads
`~/.config/ollama-forge/config.yaml` as **YAML** with *flat* keys
(`ollama_url`, `default_model`, …) — [src/lib.rs:60](src/lib.rs#L60) — and even
`--config <path>` parses YAML ([src/main.rs:1411](src/main.rs#L1411)). Net
effect: the file `forge init` produces is never read, and its key shape doesn't
match `Config` anyway. The committed [forge.toml](forge.toml) at repo root is
similarly decorative (it even sets `default_model = "llama3.2:3b"`, which
contradicts the `qwen2.5-coder:7b` default everywhere in code). **Open question
for you:** was the intent TOML-with-sections or YAML-flat? This wants
reconciling.

**Scaffolded / not-yet-wired (acknowledged in code):**
- **`forge parallel`** — intentionally errors and points to `forge build`
  ([src/main.rs:1139](src/main.rs#L1139)).
- **TDD enforcement** — `TddEnforcer` is built and held by the orchestrator but
  never invoked in the build path; `tdd_enforced` defaults to `false` and the
  comment at [src/orchestrator/mod.rs:23](src/orchestrator/mod.rs#L23) says so.
  Note the repo `forge.toml` sets `enforced = true` (moot, since it isn't read).
- **`auto_unload` / VRAM auto-unload loop** — `VramSentinel.auto_unload` is
  `#[allow(dead_code)]`, "wired in a future loop when auto-unload lands"
  ([src/monitoring/mod.rs:11](src/monitoring/mod.rs#L11)).
- **`MergingAgent`** (alternate merger with hardcoded `deepseek-coder-v2:16b`)
  and the orchestrator's `self_correct` / `model_on_model_audit` helpers
  reference hardcoded models and appear **unused by any command** — likely
  remnants/forward-looking code.
- **`build_response_schema`** in the agent is `#[allow(dead_code)]`, reserved
  for a future strict-schema mode.
- **`ExecutionConfig.parallel_workers` / executor `workers`** is a soft cap
  that is **not actually enforced** ([src/executor/mod.rs:46](src/executor/mod.rs#L46)).

**Roadmap items still open (from README "Roadmap"):** end-to-end orchestrator,
full SKILL.md convergence, GBNF grammar-constrained tool calls, airgap
installer bundle, more editor integrations (only Neovim exists), published
prebuilt binaries (the `install.sh --prebuilt` path and release workflow exist
but no `v*` tag has shipped), speculative decoding.

**Tests:** ~110 test functions across `src/` unit tests + 11
`tests/*.rs` integration files. One is gated:
[tests/structured_output.rs](tests/structured_output.rs) only runs with
`FORGE_LIVE_OLLAMA=1` (needs a live daemon). Comments mention a
`tests/tools_live.rs` gated by `FORGE_LIVE_NET=1`, but **that file does not
exist** in the tree — a stale reference in
[tests/tools_html_and_arxiv.rs](tests/tools_html_and_arxiv.rs). No `#[ignore]`
tests, no `todo!()`/`unimplemented!()` in real code paths.

**Internal "must stay in sync" coupling:** the default model ladder is
duplicated in three places that comments explicitly say must agree —
`Config::default` ([src/lib.rs:76](src/lib.rs#L76)), `STARTER_FORGE_TOML`
([src/main.rs:1364](src/main.rs#L1364)), and `OrchestratorConfig::default`
([src/orchestrator/mod.rs:48](src/orchestrator/mod.rs#L48)). `ModelConfig::default`
in the router still uses the *older* ladder (`llama3.2:3b` /
`deepseek-coder-v2:16b`) — a mild inconsistency.

**Repo metadata oddity:** `Cargo.toml` repository URL is
`github.com/pranayrishi/ollamax`, the README clone URL is
`github.com/ollama-forge/ollama-forge`, and the local directory is
`Ollama-Optimizer`. Three names for one project — worth picking one.

---

## 9. Open Questions

These are the things I genuinely can't resolve from the code alone:

1. **Config format:** should config be YAML-flat (what the loader reads) or
   sectioned TOML (what `forge init` writes)? Right now they disagree and the
   `init` output is inert. Which did you intend?
2. **Is the orchestrator path exercised against real workloads?** README marks
   `forge build` ✅ but the roadmap's first open item is "wire the orchestrator
   end-to-end," and CHANGELOG calls a real `forge build` against a 50-file repo
   "intentionally out-of-scope." How finished do you consider it?
3. **Dead-but-present code:** are `MergingAgent`, `self_correct`,
   `model_on_model_audit`, and `ModelfileGenerator` deliberately kept for a
   planned feature, or are they cruft safe to delete?
4. **Canonical repo name/URL** — `ollamax`, `ollama-forge`, or
   `Ollama-Optimizer`? The mismatch affects install instructions and the
   release/`--prebuilt` URL.
5. **Router model ladder** — should `ModelConfig::default` be updated to the
   qwen2.5-coder ladder used everywhere else, or is the llama/deepseek set
   intentional for routing?
6. **Missing `tests/tools_live.rs`** — was a live-network test suite planned
   (the comment references `FORGE_LIVE_NET=1`) and never committed, or was that
   comment left over from a refactor?

---

*End of analysis. The fastest re-entry point is `forge status` (cheapest way to
confirm your install works), then skim [CHANGELOG.md](CHANGELOG.md) "session 7"
for the most recent work, then [src/main.rs](src/main.rs) to see every command
body in one place.*
