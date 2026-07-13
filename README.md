# Ollamax

**A harness optimization layer for local coding agents that run on Ollama.**

> [简体中文](README.zh.md) · [日本語](README.ja.md) · [Deutsch](README.de.md) · [Português](README.pt.md)

> **Status: pre-alpha (v0.1.0).** The CLI compiles, ships hardware detection,
> bundled skill recipes, and a security/secret scanner. Parallel orchestration,
> the build pipeline, and the skills marketplace are scaffolded but not yet
> wired end-to-end. See [Roadmap](#roadmap) for what works today vs. what
> doesn't. PRs welcome — [good-first-issue label](https://github.com/pranayrishi/ollamax/labels/good%20first%20issue).

---

## Why this exists

If you want AI coding assistance without shipping your codebase to a third
party, your options today are: configure each tool (Aider, Continue.dev, Cline,
OpenHands, twinny…) by hand, juggle model selection per task, manage VRAM
manually, and accept whatever defaults each tool ships. Ollamax is a
local-first coding agent and workspace console built around Ollama:

- **Hardware-aware defaults.** Detects RAM/VRAM at install and runtime, picks a
  sane default model and `num_ctx`, refuses to load models that won't fit.
- **`keep_alive` discipline.** Long-lived warm models so the second prompt
  isn't a 15-second cold load.
- **Bundled, auditable skill recipes.** JSON files in `skills/recipes/` —
  read them, fork them, ship your own.
- **Local secret scanner.** Refuses to send files containing private keys, AWS
  keys, GitHub tokens, or known credential patterns to the model.

It is pre-alpha, but its Agent mode now works on the codebase you explicitly
open: it can inspect files, edit them through sandboxed tools, run bounded
validation commands, and show/require approvals.

---

## Honest comparison

| Feature                          | Claude Code   | Aider        | Continue.dev | **Ollama-Forge**          |
| :------------------------------- | :-----------: | :----------: | :----------: | :-----------------------: |
| Runs offline                     | ❌            | ✅ (w/ local) | ✅ (w/ local) | ✅                        |
| Built for local-first            | ❌            | partial      | partial      | ✅                        |
| Hardware-aware model selection   | n/a           | ❌           | ❌           | ✅                        |
| Bundled secret scanner           | ❌            | ❌           | ❌           | ✅                        |
| Multi-agent parallel execution   | ✅            | ❌           | ❌           | 🚧 scaffolded             |
| LoRA fine-tune on your code      | ❌ (impossible) | ❌         | ❌           | 🚧 planned                |
| Mature, daily-driver ready       | ✅            | ✅           | ✅           | ❌ (pre-alpha)            |

If you need something that works *today*, use Aider or Continue. If you want
to help build the harness layer they're missing, read on.

---

## Quick start

Requires [Ollama](https://ollama.com/download) installed and `ollama serve`
running. Rust toolchain (1.75+) needed to build from source — pre-built
binaries are not yet shipping.

```bash
git clone https://github.com/pranayrishi/ollamax.git
cd ollamax
./install.sh                      # builds with cargo, installs to ~/.local/forge/bin
forge status                      # show detected hardware + recommended model
forge status --models             # also lists models known to Ollama
```

`forge status` is the most useful command in v0.1.0. It is also the cheapest
way to verify your install works without pulling a model.

---

## Local workspace Agent

Run the Agent from the root of the project you want it to change:

```bash
forge agent "Add a health endpoint and tests for it"
```

The default `confirm` mode shows a plan and asks before each file write, exact
edit, or shell validation command. `--autonomy readonly` lets it inspect and
search only; `--yes` (or `--autonomy auto`) permits actions within the current
workspace sandbox without per-step prompts.

```bash
forge agent --autonomy readonly "Explain the authentication flow"
forge agent --yes "Create a small CLI command and run its unit tests"
```

The agent has explicit `fs_list`, `fs_search`, `fs_read`, `fs_write`, and
`fs_edit` tools. It rejects absolute paths, `..` traversal, and symlink paths;
write/edit tools are bounded in size and stay under the captured workspace
root. Local coding runs do not register web or MCP tools by default.

## Local Agent Console

Start the local server from the workspace and open the printed local URL with
`/console` appended:

```bash
forge serve --port 7878
# open http://127.0.0.1:7878/console
```

The console is a local task board with queued/working/review/done states, a
model and permission picker, streamed activity, plans, file-change records,
and approval controls. Task snapshots are scoped to the server's workspace in
your browser storage. The server binds only to loopback and issues a
per-process capability to its trusted desktop, VS Code, and console clients;
do not expose the local port to an untrusted reverse proxy.

---

## What works in v0.1.0

| Command                      | Status | What it does                                                                                  |
| :--------------------------- | :----: | :-------------------------------------------------------------------------------------------- |
| `forge research "<q>"`       |   ✅   | **Tool-using research agent.** Local Ollama + free public tools (DuckDuckGo, Wikipedia, arXiv, plain HTTP). Configurable `--max-iterations`. Honors `FORGE_TRACE_WIDTH`. |
| `forge tools`                |   ✅   | Lists the four bundled tools and the JSON schema the agent uses to call them                 |
| `forge replay <log>`         |   ✅   | Re-issues every Ollama call in a JSONL replay log against locally-installed models, reports hash drift |
| `forge instincts [<log>]`    |   ✅   | **Continuous learning.** Surfaces repeated tasks/system prompts from your replay log as candidate skills/rules. Read-only. `--threshold N` to lower the floor. |
| `forge rules list/init/show/path` | ✅ | **Persistent always-rules.** Drop Markdown files into `~/.config/ollama-forge/rules/` and they get prepended to every system prompt across `chat`, `research`, `run-skill`, `analyze`, `test`, and `build`. |
| `forge build "..." -o dir/`  |   ✅   | Heterogeneous parallel orchestration with per-worker progress events; `--output` extracts labeled code blocks (incl. small-model "path on first line inside block" shape) and writes them to disk. Reports total tokens + duration. |
| `forge status`               |   ✅   | Hardware (NVIDIA / AMD / Apple Silicon / Intel / CPU), recommended model, Ollama health, loaded models |
| `forge --version`            |   ✅   | Includes git short SHA so a build can be pinned for replay/debug                              |
| `forge optimize`             |   ✅   | Prints a tuned `Modelfile` for your hardware                                                  |
| `forge audit <dir>`          |   ✅   | Walks the dir, runs the secret scanner, exits 1 on Critical/High; `--json` for CI consumers   |
| `forge preload [model]`      |   ✅   | Warm-loads a model with `--keep-alive`; braille spinner so a 14B cold-load doesn't look hung |
| `forge chat "..."`           |   ✅   | Streams tokens to stdout as they arrive (real NDJSON drain, not buffered)                     |
| `forge agent "..."`          |   ✅   | Local workspace agent: lists/searches/reads files, writes through a sandbox, and validates changes with visible approval controls |
| `forge serve --port 7878`    |   ✅   | Runs the local desktop/VS Code backend and the browser-based Agent Console at `/console`       |
| `forge run-skill <name>`     |   ✅   | Loads a skill, picks an installed model (with fallback), streams the response                |
| `forge skills list/add/search` | ✅   | Recipe management; `add` is local-path only by design                                         |
| `forge analyze <dir>`        |   ✅   | Local secret scan + token-budgeted model code review                                          |
| `forge test <file>`          |   ✅   | Generates tests for a single source file in the right framework for the language              |
| `forge init`                 |   ✅   | Writes a supported project-local `forge.toml`                                                 |
| `forge parallel`             |   ❌   | Errors loudly — use `forge build`                                                             |

✅ = works · 🟢 = works but unproven against real-world workloads · ❌ = not implemented (and tells you so)

### Research agent

`forge research "<question>"` runs a tool-using agent loop entirely on
local Ollama + free public tools. No paid APIs. No keys. Example:

```bash
forge research "what is the airspeed velocity of a barn swallow" --trace
```

Bundled tools (all free, all keyless):

| Tool          | Source                                | Purpose                                          |
| :------------ | :------------------------------------ | :----------------------------------------------- |
| `web_search`  | DuckDuckGo Instant Answer JSON API    | General factual queries, definition discovery    |
| `wikipedia`   | Wikipedia REST `summary` + opensearch | Direct article lookups + fuzzy title search      |
| `arxiv`       | arXiv Atom API                        | Academic papers (top 5 hits, with PDF links)     |
| `fetch_url`   | plain HTTP GET                        | Read a specific page after discovering its URL   |

The agent runs in a JSON-constrained loop: each iteration the model emits
either `{action: "use_tool", tool, args}` or `{action: "answer", text}`,
the loop dispatches accordingly, and tool results are fed back into the
transcript. Capped at `--max-iterations` (default 6). Per-tool rate limit
of 250 ms prevents the loop from hammering DDG into a temporary IP ban.

### Heterogeneous parallel execution

`forge build` runs multiple workers in parallel on **different models at
the same time**. Architecture work routes to the largest installed model,
boilerplate (frontend / tests) routes to the smallest, balanced work uses
the analyzer's pick. Distinct models are preloaded *concurrently* into
Ollama (which serializes per-model but not across models), so a 32B and a
3B can warm up at the same time.

The router is **VRAM-aware**: if the sum of selected models wouldn't fit
in `free_vram_mb`, the router collapses the assignment to the largest
single model that does fit. No silent OOM kills mid-build.

`forge build "..." --output ./generated/` extracts every labeled fenced
code block (e.g., ` ```rust src/main.rs ` or ` ```yaml file=.github/workflows/ci.yml `)
and writes it to the right path under `./generated/`. Path traversal
(`..`) and absolute paths are rejected.

### Deterministic replay (compliance / audit trail)

Set `FORGE_REPLAY_LOG=path/to/log.jsonl` and every Ollama call from
`forge chat` and `forge research` is appended to the log with:

- the `model_digest` from `/api/tags` (so a future `ollama pull` of the
  same tag is detectable),
- the seed, temperature, top_p, num_ctx, system prompt, and prompt itself,
- a real **SHA-256** of the prompt and response (not a `DefaultHasher`
  shim that drifts across Rust versions),
- the `forge_version` string including the git SHA.

Then `forge replay path/to/log.jsonl` re-issues every call against
locally-installed models and reports any hash drift. With `seed=0` and
`temperature=0` (which `chat` switches to automatically when the env var
is set), this gives bit-identical replays of past sessions, forever, on
the user's own hardware. **No hosted tool can offer this** because
providers rotate weights silently. The compliance pitch — finance,
healthcare, defense, legal — depends on this property.

### Schema-constrained output

`OllamaProvider::generate` accepts an Ollama `format` parameter (v0.5+),
either the literal `"json"` for free-form valid JSON or a full JSON Schema
for constrained decoding. This is the local-LLM equivalent of OpenAI's
`response_format` and the closest thing forge has to "guaranteed-valid tool
calls" without dropping into raw GBNF grammars. There is a live integration
test in [`tests/structured_output.rs`](tests/structured_output.rs) gated by
`FORGE_LIVE_OLLAMA=1`.

### SKILL.md compatibility

Drop a `<name>/SKILL.md` (Anthropic YAML-frontmatter format) into your
skills dir and `forge run-skill` will load it alongside the JSON recipes.
See [`tests/skill_md_compat.rs`](tests/skill_md_compat.rs) for the
supported frontmatter shape.

---

## Architecture (today, not aspirational)

```
src/
├── cli/          clap subcommands
├── orchestrator/ planner→executor→merger pipeline (scaffold)
├── router/       complexity heuristic → model tier selection
├── executor/     parallel worker pool (scaffold)
├── context/      sliding-window context manager + Modelfile generator
├── providers/    Ollama HTTP client (`/api/generate`, `/api/chat`, `/api/tags`)
├── security/     regex secret scanner + audit log
├── monitoring/   sysinfo-based hardware detection (VRAM detection is best-effort)
└── skills/       JSON recipe loader
```

Module boundaries are stable; internals will change.

---

## Configuration

`forge init` writes `forge.toml`:

```toml
[forge]
version = "1.0"

[ollama]
url = "http://127.0.0.1:11434"
default_model = "llama3.2:3b"
planning_model = "qwen2.5-coder:7b"

[execution]
parallel_workers = 4
max_context_tokens = 32768

[security]
enabled = true
scan_secrets = true

[tdd]
enforced = true
```

Defaults are picked for an 8 GB VRAM machine. `forge status` will tell you
whether your hardware can do better.

`forge.toml` is a project-local override and is loaded automatically from the
current directory. You can also select it explicitly with
`forge --config path/to/forge.toml status`. Existing global
`~/.config/ollama-forge/config.yaml` files remain supported; project TOML
values override the corresponding global settings.

### Windows and custom Ollama hosts

Ollamax defaults to `http://127.0.0.1:11434` rather than `localhost` to avoid
Windows installations where `localhost` resolves to IPv6 but Ollama only
listens on IPv4. If your Ollama daemon uses another local host or port, set
`OLLAMA_HOST` before launching Ollamax, for example:

```powershell
$env:OLLAMA_HOST = "127.0.0.1:11555"
ollama serve
```

You can instead set `[ollama].url` in `forge.toml`. When connectivity fails,
the model picker now reports the exact endpoint and underlying Ollama error;
on Windows, check it directly with
`Invoke-RestMethod http://127.0.0.1:11434/api/tags`.

---

## Skills

Skills are plain JSON files in `skills/recipes/`. Bundled today:

- `docker-expert.json` — Dockerfile multi-stage builds, k8s manifests
- `security-auditor.json` — vulnerability scanning workflow
- `react-native-expert.json` — RN component patterns
- `api-designer.json` — REST/GraphQL design

The schema is documented by [`src/skills/mod.rs`](src/skills/mod.rs). Tests
in [`tests/skill_recipes_parse.rs`](tests/skill_recipes_parse.rs) verify every
bundled recipe parses.

> **Note:** the format is intentionally close to but not yet identical to
> Anthropic's `SKILL.md` YAML-frontmatter standard. Convergence is tracked in
> issue #TBD.

---

## Privacy

All inference goes to your local Ollama daemon at `http://127.0.0.1:11434` by
default. The workspace Agent and local console register no web or MCP tools by
default, so code-agent work stays on-device. The research command and an
explicit web-tools option can contact their named public sources; the server
discloses that egress in the task stream. The secret scanner runs *before*
content is sent to the model to prevent accidental leakage of credentials in
your codebase.

This is verifiable — there are exactly two places `reqwest` is constructed
([`src/providers/ollama.rs`](src/providers/ollama.rs)), both pointing at the
configured `ollama_url`.

---

## Roadmap

Honest list. Not sorted by hype.

- [ ] Wire the orchestrator end-to-end (planner → parallel executor → merger)
- [ ] `SKILL.md` YAML format compatibility
- [ ] GBNF grammar-constrained tool calls (llama.cpp feature exposure)
- [ ] Deterministic replay log (model digest + seed + prompt hash)
- [ ] LoRA fine-tune skill (Unsloth integration)
- [ ] Airgap installer bundle (binary + Ollama + default GGUF in one tarball)
- [ ] Editor integrations: Neovim → VS Code → Zed
- [ ] Pre-built release binaries (Linux/macOS aarch64 + x86_64)
- [ ] Speculative decoding (blocked on Ollama API exposure)

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). The fastest way to help right now is
to file issues for any rough edge in `forge status` or `forge optimize` —
those are the surfaces most likely to ship in v0.2.0.

---

## License

MIT — see [LICENSE](LICENSE).
