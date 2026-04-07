# Changelog

All notable changes to Ollama-Forge are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This project does
**not** yet follow semantic versioning — every 0.x release may break things.

## [Unreleased]

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
