# Ollamax

**A harness optimization layer for local coding agents that run on Ollama.**

> [简体中文](README.zh.md) · [日本語](README.ja.md) · [Deutsch](README.de.md) · [Português](README.pt.md)

> **Status: pre-alpha.** This source tree ships hardware detection, bundled
> skill recipes, a security/secret scanner, controlled local Agent/Team
> workspace editing, a reviewed local-model catalog, explicit local voice and
> screen-region context in the Electron app, curated documentation-only GitHub
> knowledge plugins, and local evaluation records. The legacy parallel build
> path remains text-oriented; worktrees, executable plugins, and an evaluation
> runner are not yet shipped. The public `v0.2.0` download predates the local
> voice, spatial-context, and expanded-model work described below; it must not
> be treated as containing it. See [Release sequence](#release-sequence) and
> [Roadmap](#roadmap) for current limits. PRs welcome —
> [good-first-issue label](https://github.com/pranayrishi/ollamax/labels/good%20first%20issue).

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
open: it can inspect files and edit them through workspace-confined tools, run
guardrailed host-shell validation commands, and show/require approvals. The
host shell is not an OS-level sandbox.

---

## Honest comparison

| Feature                          | Claude Code   | Aider        | Continue.dev | **Ollama-Forge**          |
| :------------------------------- | :-----------: | :----------: | :----------: | :-----------------------: |
| Runs offline                     | ❌            | ✅ (w/ local) | ✅ (w/ local) | ✅                        |
| Built for local-first            | ❌            | partial      | partial      | ✅                        |
| Hardware-aware model selection   | n/a           | ❌           | ❌           | ✅                        |
| Bundled secret scanner           | ❌            | ❌           | ❌           | ✅                        |
| Bounded multi-agent roles        | ✅            | ❌           | ❌           | ✅ (one writer; parallel scouts optional) |
| LoRA fine-tune on your code      | ❌ (impossible) | ❌         | ❌           | 🚧 planned                |
| Mature, daily-driver ready       | ✅            | ✅           | ✅           | ❌ (pre-alpha)            |

If you need something that works *today*, use Aider or Continue. If you want
to help build the harness layer they're missing, read on.

---

## Quick start

Requires [Ollama](https://ollama.com/download) installed and `ollama serve`
running. Rust toolchain (1.75+) is needed to build from source. Tagged releases
publish CLI, VS Code, and desktop artifacts through the
[Ollamax releases repository](https://github.com/pranayrishi/ollamax-releases).

```bash
git clone https://github.com/pranayrishi/ollamax.git
cd ollamax
./install.sh                      # builds with cargo, installs to ~/.local/forge/bin
forge status                      # show detected hardware + recommended model
forge status --models             # also lists models known to Ollama
```

`forge status` is the cheapest way to verify a source or packaged install
without pulling a model.

## Model catalog and local runtimes

`forge models` distinguishes models that can be pulled by a local Ollama
daemon from models that need separately operated server infrastructure. It only
prints an `ollama pull` command for a reviewed Ollama-local tag; it does not
present a cloud service or a server-class checkpoint as a laptop download.

For a new local setup, start with a model that fits your hardware and leave
room for the context cache:

```bash
ollama pull qwen3.5:4b       # compact local generalist / visual model
ollama pull gemma4:e2b       # modest local Gemma 4 visual model
ollama pull deepseek-r1:8b   # local DeepSeek reasoning model; text-only
forge models
```

Qwen 3.5 and Gemma 4 are the current consumer-local visual options in the
catalog. DeepSeek-R1 `:8b` is a useful local reasoning partner, but it cannot
read a screen crop on its own, so pair it with an installed vision-capable
model for spatial work. Hardware recommendations are conservative estimates,
not a promise that a model's maximum context will fit alongside other loaded
models.

DeepSeek V4 Flash/Pro and MiniMax M3 are shown as **separately self-hosted,
server-class** options. They are not Ollama pulls and are not casual offline
laptop installs. Running either one requires an appropriately provisioned,
locally operated inference server (for example a compatible vLLM, SGLang, or
Transformers deployment). Ollamax routes an **explicitly configured,
loopback-only** OpenAI-compatible endpoint in Chat, Agent, Research, and Team;
the managed local server and desktop model picker use the same selector. It
does not provision, size, or operate that server, and Auto routing never
silently sends a request there. Declare the endpoint and one or more served
models, then select a `local:<endpoint>/<model>` name:

```toml
[[local_endpoints]]
id = "lab"
url = "http://127.0.0.1:8000" # normalized and restricted to loopback /v1
max_parallel_requests = 2

[[local_endpoints.models]]
id = "deepseek-v4-flash"
served_model = "DeepSeek-V4-Flash"
label = "Lab DeepSeek V4 Flash"
thinking = true
context_window_tokens = 32768
```

```bash
forge chat --model local:lab/deepseek-v4-flash "Summarize this design"
forge agent --model local:lab/deepseek-v4-flash "Implement the approved plan"
forge team --model local:lab/deepseek-v4-flash --parallel-scouts "Add tests"
```

An optional `api_key_env` may name a local server bearer-token environment
variable; tokens are never stored in `forge.toml` or displayed by the picker.
Configured endpoints share a bounded request lane, including the parallel
read-only scout phase. Build/Orchestrator remains Ollama-only for now and
rejects `local:` selectors clearly rather than pretending it can launch or
manage a server-class runtime. Direct catalog names for separately self-hosted
models are rejected with the same `local:<endpoint>/<model>` instruction,
rather than being sent to Ollama. A cataloged cloud-only tag such as
`minimax-m3:cloud` is also rejected before any Ollama request; it cannot be
used as a substitute for a self-hosted model. More generally, a direct
case-insensitive `:cloud` suffix (for example `qwen3.5:cloud` or
`gemma4:cloud`) is rejected rather than being mistaken for an offline model.
The catalog also discloses
`minimax-m3:cloud` as cloud-only specifically so it cannot be mistaken for a
free offline model. Review each model's license and deployment notes before
commercial use—MiniMax M3 in particular has notice requirements.

The catalog is based on the current upstream model pages: [Qwen 3.5 on
Ollama](https://registry.ollama.com/library/qwen3.5), [Gemma 4 on
Ollama](https://registry.ollama.com/library/gemma4), [DeepSeek V4
Flash](https://huggingface.co/deepseek-ai/DeepSeek-V4-Flash), and [MiniMax
M3](https://huggingface.co/MiniMaxAI/MiniMax-M3). Those sources are also the
place to check model licenses, hardware requirements, and deployment guidance
before changing a local endpoint.

## Release sequence

The public release is assembled in two deliberately ordered tag workflows. Do
not update website download links or describe a public version as containing a
feature until both workflows have completed and the public release is visible.

```bash
# 1. Build and attach the Electron installers to a draft for this version.
git tag app-vX.Y.Z
git push origin app-vX.Y.Z

# 2. After the app workflow succeeds, build the CLI/VS Code bundles, verify the
#    complete asset contract, and publish that same draft.
git tag vX.Y.Z
git push origin vX.Y.Z
```

`app-vX.Y.Z` is an internal staging tag, not a public download release. The
following `vX.Y.Z` tag publishes only after the desktop and CLI/VS Code assets
are present. This is why the existing public `v0.2.0` assets remain a baseline
until a newer pair of workflows has passed.

---

## Local workspace Agent

Run the Agent from the root of the project you want it to change:

```bash
forge agent "Add a health endpoint and tests for it"
```

The default `confirm` mode shows a plan and asks before each file write, exact
edit, or shell validation command. `--autonomy readonly` lets it inspect and
search only; `--yes` (or `--autonomy auto`) permits workspace-confined file
actions without per-step prompts. Shell commands start in the workspace but
are guardrailed host-shell commands, not an OS-level sandbox.

```bash
forge agent --autonomy readonly "Explain the authentication flow"
forge agent --yes "Create a small CLI command and run its unit tests"
```

The agent has explicit `fs_list`, `fs_search`, `fs_read`, `fs_write`, and
`fs_edit` tools. It rejects absolute paths and `..` traversal; each agent run
pins a descriptor-relative workspace capability, so a later symlink or
workspace-root path swap cannot redirect a read or write outside the selected
root. Write/edit tools are bounded in size. Local coding runs do not register
web or MCP tools by default.

On macOS and Linux, approved shell validation and Team diff review also enter
the captured workspace by descriptor; if the visible workspace root has been
replaced, the command fails closed. This still is not an OS/container sandbox.

## Controlled local Team

For a complex workspace change, use the bounded team coordinator instead of
the legacy text-only build workflow:

```bash
forge team "Add an authenticated health endpoint and its tests"
forge team --parallel-scouts --scout-model qwen3.5:4b \
  --planner-model deepseek-r1:8b --yes "Refactor the API boundary"
```

The default topology is intentionally conservative: two read-only scouts, a
read-only planner hand-off, one writer, fixed repository-detected checks, and
an advisory reviewer. The worker executor enforces the configured concurrency
limit, tracks pending/running/terminal worker state, and returns results in
input order; parallel work is real but bounded. The writer is always
single-lane. `--parallel-scouts` only overlaps the two read-only scouts when
your project enables at least two workers; it never enables simultaneous
writers in one checkout. This is intentional coordination, not a collection of
independent worktrees. A `Verified` result requires a successful writer
mutation plus a passing functional test command (`cargo test --workspace`,
`npm test`, or `python -m pytest`) after all detected checks pass. A
diff/lint/typecheck-only run is reported as `ChecksPassed` and still needs human
acceptance. Neither status substitutes for CI, security review, or deployment
validation. Team runs are bounded/restartable rather than an unattended
infinite process. In `--autonomy auto`, checks execute project code on the
host, so use confirmation for unfamiliar repositories.

The local server exposes the same workflow at `POST /api/team`; the VS Code
extension, standalone app, and browser console include a Team mode with
streamed role and verification events.

## Curated GitHub knowledge plugins

Knowledge plugins let an installed local model receive bounded reference
documentation from a curated repository without treating that repository as
executable code:

```bash
forge plugins list
forge plugins install roboflow-supervision
forge plugins context "track objects in a Python video pipeline"
forge plugins remove roboflow-supervision
```

Installation fetches repository metadata and a capped README, checks the
curated star/license policy, records the default-branch commit when available,
and saves provenance plus a SHA-256 hash. It does **not** clone repositories,
install packages, execute scripts, load hooks/MCP servers, or grant tools.
Installed README text is labeled untrusted and is relevance-matched into Agent
and Team context only as reference data. This initial plugin surface is CLI
managed; a graphical marketplace/permission UI is not yet shipped. The bundled
catalog spans computer vision (Supervision/OpenCV), machine learning
(Transformers), web/API (FastAPI/Next.js), language tooling (TypeScript),
browser testing (Playwright), desktop apps (Tauri), and Python testing
(`pytest`); every install rechecks that repository's current GitHub stars and
SPDX license against its curated policy.

## Local evaluation records

Ollamax includes a local, append-only evaluation schema so model/topology
experiments can be compared honestly:

```bash
forge eval validate scenarios/greeting-fix.toml
forge eval report results/baseline.jsonl
forge eval compare results/baseline.jsonl results/team.jsonl
```

It validates declarative scenarios and scores caller-provided JSONL evidence
(verified completion, checks, duration, tokens, calls, regressions, and scope
violations). It is a scoring/comparison foundation, not yet an agent benchmark
runner; do not interpret an empty or manually supplied report as a benchmark
claim. See [the capability-gap analysis](docs/local-agent-capability-gap-analysis.md)
for the current limits and next steps.

## Local Agent Console

Start the local server from the workspace and open the printed local URL with
`/console` appended:

```bash
forge serve --port 7878
# open http://127.0.0.1:7878/console
```

The console is a local task board with Agent and Coding Team modes,
queued/working/review/done states, a model and permission picker, streamed
activity, plans, file-change records, verifier evidence, and approval controls.
Task snapshots are scoped to the server's workspace in your browser storage.
The server binds only to loopback and issues a
per-process capability to its trusted desktop, VS Code, and console clients;
do not expose the local port to an untrusted reverse proxy.

## Desktop voice and spatial context

The Electron desktop app adopts the useful interaction shape of a cursor-side
assistant without requiring paid speech services: explicit push-to-talk records
audio only after the user holds the control, then sends the resulting WAV to a
local `whisper.cpp` executable and local model when one is configured. Optional
spoken responses use a local system voice (`say` on macOS, SAPI on Windows, or
`espeak` where available), or an explicitly configured local TTS executable.
There is no hidden cloud STT/TTS fallback. If a release has not staged a
Whisper runtime and model, the voice control says so and remains unavailable
until local setup is completed.

**Select region** opens an explicit lasso overlay. A screenshot is captured
only after that action, then the chosen display region is cropped and size
capped before it is attached as visual context. The app does not send the full
desktop to the model, does not save the crop as a workspace file, and clears
screen-derived visual briefs after the turn; those briefs are not added to
Ollamax memory or replay logs. Spatial context requires a loopback Ollama
endpoint and an installed vision model. In Chat, automatic routing selects an
installed local vision model. In Agent and Team, a separate local vision worker
first produces an untrusted evidence brief for the coding model.

A visual brief can describe UI evidence—it cannot click the operating system,
read outside the selected crop, alter a project, or bypass Agent/Team approval
and workspace boundaries. For example, selecting a search bar can help an
Agent identify its layout before implementing a requested equivalent, but file
writes still follow the selected autonomy mode and normal confirmation flow.

---

## What is implemented in this source tree

The table describes the current source tree. It is not a claim about the
already-published `v0.2.0` artifacts; use the [release sequence](#release-sequence)
before representing a newer feature as downloadable.

| Command                      | Status | What it does                                                                                  |
| :--------------------------- | :----: | :-------------------------------------------------------------------------------------------- |
| `forge research "<q>"`       |   ✅   | **Tool-using research agent.** Local Ollama + free public tools (DuckDuckGo, Wikipedia, arXiv, plain HTTP). Configurable `--max-iterations`. Honors `FORGE_TRACE_WIDTH`. |
| `forge tools`                |   ✅   | Lists the four bundled tools and the JSON schema the agent uses to call them                 |
| `forge replay <log>`         |   ✅   | Re-issues every Ollama call in a JSONL replay log against locally-installed models, reports hash drift |
| `forge instincts [<log>]`    |   ✅   | **Continuous learning.** Surfaces repeated tasks/system prompts from your replay log as candidate skills/rules. Read-only. `--threshold N` to lower the floor. |
| `forge rules list/init/show/path` | ✅ | **Persistent always-rules.** Drop Markdown files into `~/.config/ollama-forge/rules/` and they get prepended to every system prompt across `chat`, `research`, `run-skill`, `analyze`, `test`, and `build`. |
| `forge build "..." -o dir/`  |   ✅   | Legacy text-oriented parallel orchestration; `--output` extracts labeled code blocks and writes them to disk. Use `forge team` for controlled workspace edits. |
| `forge status`               |   ✅   | Hardware (NVIDIA / AMD / Apple Silicon / Intel / CPU), recommended model, Ollama health, loaded models |
| `forge models`               |   ✅   | Reviewed Qwen, Gemma 4, DeepSeek, and MiniMax catalog with explicit Ollama-local, self-hosted, and cloud-only labels; only local Ollama entries get pull commands |
| `forge --version`            |   ✅   | Includes git short SHA so a build can be pinned for replay/debug                              |
| `forge optimize`             |   ✅   | Prints a tuned `Modelfile` for your hardware                                                  |
| `forge audit <dir>`          |   ✅   | Walks the dir, runs the secret scanner, exits 1 on Critical/High; `--json` for CI consumers   |
| `forge preload [model]`      |   ✅   | Warm-loads a model with `--keep-alive`; braille spinner so a 14B cold-load doesn't look hung |
| `forge chat "..."`           |   ✅   | Streams tokens to stdout as they arrive (real NDJSON drain, not buffered)                     |
| `forge agent "..."`          |   ✅   | Local workspace agent: lists/searches/reads files, writes through a workspace-confined filesystem boundary, and validates with approval controls (host shell is not OS-sandboxed) |
| `forge team "..."`           |   ✅   | Read-only scouts + planner, one controlled writer, repository-detected verification, bounded repair, and advisory review; optional read-only parallel scouts |
| `forge plugins ...`           |   ✅   | Curated GitHub knowledge-document installs with provenance, policy checks, integrity hashes, and untrusted-context framing; no repository code execution |
| `forge eval validate/report/compare` | ✅ | Validates local scenarios and scores/compares append-only JSONL evidence; not an evaluation runner yet |
| `forge serve --port 7878`    |   ✅   | Runs the local desktop/VS Code backend and browser-based Agent/Team Console at `/console`       |
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

`forge build` runs multiple workers in parallel on **different installed
models**. Architecture work routes to the largest installed model, boilerplate
(frontend / tests) routes to the smallest, and balanced work uses the
analyzer's pick. Distinct models are preloaded in a separately bounded lane,
while execution workers obey the configured concurrency cap. Ollama may still
serialize work per model and hardware can be the practical bottleneck.

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
├── agent/        bounded local tool-calling workspace agent
├── team/         read-only scouts, single writer, verifier, reviewer
├── plugins/      curated documentation-only GitHub knowledge installs
├── evals/        local scenario/evidence validation and comparison
├── tools/        descriptor-confined workspace tools and guarded shell
├── orchestrator/ planner→executor→merger pipeline (scaffold)
├── router/       complexity heuristic → model tier selection
├── executor/     bounded parallel worker + preload pools
├── context/      sliding-window context manager + Modelfile generator
├── providers/    Ollama client + strict loopback OpenAI-compatible client
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
default_model = "qwen3.5:4b"
planning_model = "deepseek-r1:8b"
execution_models = ["qwen3.5:4b", "deepseek-r1:8b", "qwen3.5:9b", "gemma4:12b"]

[execution]
parallel_workers = 4
max_context_tokens = 32768

[security]
enabled = true
scan_secrets = true

[tdd]
enforced = true
```

The included starter file is a balanced example, not a promise that every
listed model fits an 8 GB GPU at once. `forge status` and `forge models` expose
the detected hardware and catalog caveats; leave headroom for the context cache
and prefer one small visual model plus one text/reasoning model on consumer
hardware.

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

By default, inference goes to your local Ollama daemon at
`http://127.0.0.1:11434`. The optional OpenAI-compatible provider is deliberately
loopback-only: it rejects remote hosts and is for a separately operated local
inference server, not an internet API. The workspace Agent and local console
register no web or MCP tools by default, so code-agent work stays on-device.
The research command and an explicit web-tools option can contact their named
public sources; the server discloses that egress in the task stream. The secret
scanner runs *before* content is sent to the model to prevent accidental
leakage of credentials in your codebase.

Voice recognition and speech output are also local-only as described in
[Desktop voice and spatial context](#desktop-voice-and-spatial-context).
Screen-region context is limited to an explicit user selection and, for visual
requests, is rejected unless the Ollama endpoint is loopback. A cloud-only
catalog entry remains a disclosure, not a local routing target.

---

## Roadmap

Honest list. Not sorted by hype.

- [ ] Complete the legacy orchestrator's end-to-end planner → executor → merger workflow
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
those are the surfaces most likely to change next.

---

## License

MIT — see [LICENSE](LICENSE).
