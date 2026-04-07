# Ollama-Forge

**A harness optimization layer for local coding agents that run on Ollama.**

> **Status: pre-alpha (v0.1.0).** The CLI compiles, ships hardware detection,
> bundled skill recipes, and a security/secret scanner. Parallel orchestration,
> the build pipeline, and the skills marketplace are scaffolded but not yet
> wired end-to-end. See [Roadmap](#roadmap) for what works today vs. what
> doesn't. PRs welcome — [good-first-issue label](https://github.com/ollama-forge/ollama-forge/labels/good%20first%20issue).

---

## Why this exists

If you want AI coding assistance without shipping your codebase to a third
party, your options today are: configure each tool (Aider, Continue.dev, Cline,
OpenHands, twinny…) by hand, juggle model selection per task, manage VRAM
manually, and accept whatever defaults each tool ships. Ollama-Forge is the
shared optimization layer underneath:

- **Hardware-aware defaults.** Detects RAM/VRAM at install and runtime, picks a
  sane default model and `num_ctx`, refuses to load models that won't fit.
- **`keep_alive` discipline.** Long-lived warm models so the second prompt
  isn't a 15-second cold load.
- **Bundled, auditable skill recipes.** JSON files in `skills/recipes/` —
  read them, fork them, ship your own.
- **Local secret scanner.** Refuses to send files containing private keys, AWS
  keys, GitHub tokens, or known credential patterns to the model.

It is **not** a replacement for Aider or Cline — it's the glue that makes them
sing on local hardware.

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
git clone https://github.com/ollama-forge/ollama-forge
cd ollama-forge
./install.sh                      # builds with cargo, installs to ~/.local/forge/bin
forge status                      # show detected hardware + recommended model
forge status --models             # also lists models known to Ollama
```

`forge status` is the most useful command in v0.1.0. It is also the cheapest
way to verify your install works without pulling a model.

---

## What works in v0.1.0

| Command                      | Status | What it does                                                                                  |
| :--------------------------- | :----: | :-------------------------------------------------------------------------------------------- |
| `forge status`               |   ✅   | Hardware detection (NVIDIA / AMD / Apple Silicon / Intel / CPU), recommended model, Ollama health, currently-loaded models |
| `forge --version`            |   ✅   | Includes git short SHA so a build can be pinned for replay/debug                              |
| `forge optimize`             |   ✅   | Prints a tuned `Modelfile` for your hardware                                                  |
| `forge audit <dir>`          |   ✅   | Walks the dir, runs the secret scanner, exits 1 on Critical/High; `--json` for CI consumers   |
| `forge preload [model]`      |   ✅   | Warm-loads a model with `--keep-alive`; braille spinner so a 14B cold-load doesn't look hung |
| `forge chat "..."`           |   ✅   | Streams tokens to stdout as they arrive (real NDJSON drain, not buffered)                     |
| `forge run-skill <name>`     |   ✅   | Loads a skill, picks an installed model (with fallback), streams the response                |
| `forge skills list`          |   ✅   | Lists installed skill recipes                                                                |
| `forge skills add <path>`    |   ✅   | Adds a JSON skill from a local path (no remote URLs — preserves "no network but Ollama")     |
| `forge skills search <q>`    |   ✅   | Lists *all* skills matching a name/tag/keyword                                                |
| `forge analyze <dir>`        |   ✅   | Local secret scan + token-budgeted model code review                                          |
| `forge test <file>`          |   ✅   | Generates tests for a single source file in the right framework for the language              |
| `forge build "..."`          |   🟢   | Full orchestrator: router → preload → parallel executor → section-aware merger                 |
| `forge init`                 |   🟢   | Writes a starter `forge.toml`                                                                 |
| `forge parallel`             |   ❌   | Errors loudly with "not implemented" — use `forge build`                                      |

✅ = works · 🟢 = works but unproven against real-world workloads · ❌ = not implemented (and tells you so)

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
url = "http://localhost:11434"
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

All inference goes to your local Ollama daemon at `http://localhost:11434`. The
binary makes no other outbound connections (no telemetry, no update checks, no
docs lookups). The secret scanner runs *before* content is sent to the model
to prevent accidental leakage of credentials in your codebase.

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
