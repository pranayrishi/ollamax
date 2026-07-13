# Local-agent capability gap analysis

## Scope and evidence

This is a comparison of the *agent harness around a model*, not a model-quality
comparison. Commercial capabilities below are limited to the linked vendors'
public documentation; no undocumented routing, private infrastructure, or
benchmark result is inferred. Ollamax claims are tied to the current source
tree and its deterministic tests. A local/offline product should not promise
cloud-scale concurrency simply because a hosted product offers it.

Commercial reference points:

- [Codex product page](https://openai.com/codex/) and [Codex app announcement](https://openai.com/index/introducing-the-codex-app/) document parallel worktrees/cloud environments, skills, review, and scheduled background work.
- [Claude Code agents](https://code.claude.com/docs/en/agents), [plugins](https://code.claude.com/docs/en/plugins), and [plugin marketplaces](https://code.claude.com/docs/en/plugin-marketplaces) document subagents/teams/worktrees and versioned extension distribution.
- Cursor publishes official documentation for [Background Agents](https://docs.cursor.com/background-agent), [Rules](https://docs.cursor.com/context/rules), and [MCP](https://docs.cursor.com/context/model-context-protocol). This report does not infer implementation details beyond those documented surfaces.
- The current [Devin Desktop worktree](https://docs.devin.ai/desktop/cascade/worktrees), [Skills](https://docs.devin.ai/desktop/cascade/skills), and [MCP](https://docs.devin.ai/desktop/cascade/mcp) documentation explicitly includes Git worktrees, progressive-disclosure skills, and MCP transports. The docs navigation also retains a separate “Windsurf Plugins” area; this report calls the cited desktop product **Devin Desktop** rather than assuming a broader product equivalence.

## What Ollamax actually has today

| Capability area | Verified current Ollamax surface | Gap relative to the documented commercial surface | Deliberate constraint / next meaningful increment |
| --- | --- | --- | --- |
| Workspace edits | `forge agent` and the server workspace-agent path give a local model bounded `fs_list`, `fs_search`, `fs_read`, `fs_write`, `fs_edit`, and a consent-gated shell. Each agent setup pins a descriptor-relative workspace capability; child I/O rejects absolute/traversal paths and cannot be redirected outside the selected root by a later symlink or root-path swap ([tools](../src/tools/files.rs), [CLI wiring](../src/main.rs)). | The legacy `forge build` path is still a text-oriented parallel orchestrator; it is not a general multi-agent workspace-edit system. | Prefer `forge agent`/`forge team` for changes. Retire or clearly label the legacy build path before positioning it as an autonomous coding workflow. |
| Coordinated local roles | This branch adds [`forge team`](../src/cli/mod.rs): two read-only scouts, one controlled writer, fixed repository-detected verification commands, and an advisory reviewer ([coordinator](../src/team/mod.rs)). Scouts can be concurrently requested with `--parallel-scouts`; the writer is always serial. `POST /api/team` streams role and verification events to local clients ([server](../src/server/mod.rs)). | No automated Git worktree creation, branch lifecycle, merge/conflict UI, task DAG, shared task board, or isolated candidate workspaces. By contrast, Codex documents isolated worktrees and parallel projects; Devin Desktop documents a worktree per Cascade session; Claude Code documents several parallel-work approaches. | One writer is intentional on household hardware: it prevents simultaneous edits in one checkout and avoids loading several large local models by default. The next safe step is opt-in, budgeted Git worktrees with explicit merge/review—not parallel writers in a shared directory. |
| Verification and review | Team verification selects literal conventional commands only (`cargo test --workspace`, known npm commands, `python -m pytest`, `git diff --check`) and records their results; it can perform a bounded repair pass. `Verified` additionally requires a successful writer mutation and a passing functional test command; hygiene-only evidence becomes `ChecksPassed`. The reviewer sees a bounded diff with external diff/text-conversion disabled plus verification evidence ([team verification](../src/team/mod.rs)). | No CI/PR integration, coverage policy, hermetic test environment, generalized test discovery, candidate comparison, or autonomous release action. Codex documents diff review and quality-focused testing/review, but that does not establish parity here. | Both statuses remain local evidence, not a claim that the task semantically meets every requirement. Repository test commands can execute project code, so the default confirmation gate is important for unfamiliar workspaces. |
| Instructions and reusable workflows | Persistent local rules and local `SKILL.md`/JSON skills are loaded into prompts ([rules](../src/rules/mod.rs), [skills](../src/skills/mod.rs)). This maps to the useful portion of the Skills concept without claiming feature parity. | There is no complete plugin runtime with arbitrary agents, hooks, LSP servers, scripts, or tool packages. Claude Code explicitly documents plugins composed of skills, agents, hooks, and MCP servers; Devin Desktop documents skill folders and progressive disclosure. | Reusable instructions are safe to load as data. Any executable extension needs a separate capability/permission design, not an automatic GitHub import. |
| Curated GitHub knowledge plugins | This branch adds a compile-time curated registry spanning Supervision/OpenCV, Transformers, FastAPI/Next.js, TypeScript, Playwright, Tauri, and pytest ([registry](../plugins/registry.json)), plus `forge plugins list/install/remove/context` ([CLI](../src/cli/mod.rs)). Installation fetches only GitHub metadata, a default-branch commit SHA when available, and a capped README; it enforces a per-entry star/license policy and saves provenance plus SHA-256 ([manager](../src/plugins/mod.rs)). Matching context is bounded and explicitly framed as untrusted. | It is intentionally **not** a general marketplace, repository clone, package installer, hook runner, MCP loader, or executable plugin system. It has no signed registry, maintainer review workflow, update diff/revocation process, dependency solver, or per-capability grant UI. | “High stars” is only a curation signal, never authority to execute code. Build a reviewed registry/update-and-diff flow before widening catalog breadth; require an explicit, separately audited capability manifest before any executable plugin class exists. |
| External tools / MCP | Ollamax has an allowlisted, stdio JSON-RPC MCP client that performs an initialize handshake and exposes listed tools ([MCP client](../src/mcp/mod.rs)). | It does not currently provide the HTTP/SSE transport breadth, registry/admin policy controls, or mature permission UX documented by Devin Desktop; Cursor and Claude Code also expose MCP as a product surface. Requests do not yet have a hard client timeout. | Configuring an MCP server is an explicit local trust decision. Treat it as code execution on the host; add request timeouts, transport policy, per-tool approval, and provenance before expanding the feature. |
| Context and discovery | Bounded workspace listing/search/read, optional code-graph endpoints, rules, skills, and relevant installed knowledge-plugin documents form a local context layer ([filesystem tools](../src/tools/files.rs), [agent setup](../src/server/mod.rs)). | No demonstrated always-on semantic index, cross-repository retrieval, dependency-aware impact analysis, or large-repo incremental indexing comparable to a mature IDE/cloud product. | Keep context bounded and inspectable. A local incremental symbol/reference index is a higher-value next step than feeding more unbounded repository text to a model. |
| Scheduling and observability | The local server contains an on-device scheduler tick and server-sent event streams for agent/team progress ([server](../src/server/mod.rs), [scheduler](../src/scheduler/mod.rs)). Team records role traces, model-call/token counters, verification evidence, and a reviewer result. | No durable distributed task queue, resumable isolated worker runtime, cloud background execution, comprehensive dashboard history, or resource-aware queueing. Codex publicly documents scheduled background work; that should not be read as a promise that Ollamax currently matches it. | Keep scheduled activity local and opt-in. Persisted task checkpoints, cancellation, and a resource budget are prerequisites to calling it reliable long-running work. |
| Evaluation | This branch adds declarative scenario validation and append-only JSONL scoring/comparison via `forge eval validate/report/compare` ([evaluation module](../src/evals/mod.rs), [CLI](../src/main.rs)). Records include model identity/digest, hardware fingerprint, fixed run config/budgets, base SHA, verifier evidence, duration, token/tool use, scope violations, and regressions. | It is **not** an evaluation runner yet: it does not invoke an agent, run a verifier, download a public benchmark, or automatically choose a model/topology. Therefore it produces no performance claim. | Use it to record comparable manual/controlled runs first. Next, add a fixture-based runner in an isolated workspace/container and publish only reproducible configuration + verifier evidence. |
| Safety boundary | Workspace file tools use a pinned descriptor-relative capability and refuse special files; consequential changes use approval policies; shell commands are audited and time-limited; on Unix, shell verification/review changes directory by the captured descriptor, while timeout **and task cancellation** kill its isolated process group. The server uses a local API token; GitHub knowledge documents are provenance-recorded, hash-checked, capped, redirect-free, and marked untrusted ([filesystem](../src/tools/files.rs), [shell](../src/tools/shell.rs), [plugins](../src/plugins/mod.rs)). | The shell is still a host shell with a denylist, timeout, and consent—not an OS-level sandbox. Windows can only terminate the direct shell child until a Job Object/sandbox is introduced; its shell working-directory check is fail-closed but not a replacement for a Job Object/container. A configured stdio MCP process is also a host process. This is below the configurable system-level sandboxing described for the Codex app. | Do not describe the current shell/MCP model as a security sandbox. The release-grade path is OS/container sandboxing, network/filesystem policy, tool-specific approvals, request timeouts, and redacted audit logs. |

## Branch implementation status and validation evidence

### 1. Bounded local team coordinator — implemented

The team coordinator is a real workspace-editing path, not parallel prose
generation. It creates a role plan, forces both scouts to finish before the
sole writer starts, runs deterministic verification, and makes the reviewer
advisory. Its integration test uses a fake local Ollama endpoint **and a real
temporary Cargo workspace**: the writer edits `app/src/lib.rs`, then the
coordinator runs `cargo test --workspace` and records a passing result
([test](../tests/team_workspace.rs)). The server test additionally validates
token authentication, SSE role events, a real workspace edit, and verifier
evidence ([test](../tests/server_team.rs)).

The regression suite also proves that a Git-hygiene-only run cannot become
`Verified`, a cancellation registered during model setup prevents the writer
from starting, unavailable reviewer tags are rejected, an empty confirm-mode
intent preview fails closed, a repository Git text-conversion filter is not
executed by the reviewer, an aborted Unix shell cannot leave an ordinary
background descendant behind, workspace file tools cannot read/write through
an outside symlink or after a root-path replacement, and FIFOs are rejected
without blocking the agent worker. This proves the bounded happy path and these safety
contracts—not arbitrary-model reliability or parallel-edit correctness. It is
intentionally less broad than the worktree facilities documented by Codex,
Claude Code, and Devin Desktop.

### 2. Curated GitHub knowledge plugins — implemented, documentation-only

The implementation validates the fetched repository identity, policy
thresholds, immutable commit metadata when available, README integrity, path
safety, and bounded relevance selection. Its loopback GitHub tests prove that
the normal install records provenance, rejects low-star/disallowed-license
metadata before writing, truncates oversized README content, and rejects
traversal/symlink abuse ([test](../tests/plugins.rs)). Repository files are
never cloned or executed.

That safety boundary is a feature: importing “popular GitHub code” into a
model context is useful, but silently giving repository content tool,
package-install, or shell authority would be a supply-chain vulnerability.

### 3. Local evaluation foundation — implemented, runner intentionally absent

The evaluation module validates safe IDs, paths, and declarative verifier
commands; persists JSONL records; and reports verified-completion, build/lint/
test, duration, token, tool, regression, and scope-violation metrics
([tests](../src/evals/mod.rs)). The CLI exposes validation, reporting, and
baseline/candidate comparison. It does **not** yet execute scenarios, so a
report should never call the output a benchmark result unless an external,
reproducible runner supplied the record evidence.

### 4. Current safety posture — improved, not sandbox parity

The branch closes high-risk path classes around workspace paths and skill/
plugin identifiers, and preserves user approval for writes and verification.
It does not turn the host shell or third-party MCP servers into a containment
boundary. That distinction needs to remain visible in the UI and release notes.

## Prioritized bridge plan

1. **Make the team path the primary editing path.** Route UI “build” requests
   that intend to change a workspace to the bounded agent/team flow; show the
   plan, changed files, verifier output, and explicit status. Keep legacy text
   build output visibly separate.
2. **Add opt-in isolated local worktrees.** Create one worktree per candidate
   only when RAM/VRAM and disk budgets allow; assign a scoped task, run fixed
   verification, present a diff, and require an explicit merge. Do not share a
   checkout between writers.
3. **Build an execution-grade evaluation runner.** Start with small, frozen
   fixtures and a fixed verifier in a temporary worktree/container. Record
   base SHA, model digest, model/context/temperature/budget, hardware, and
   exact verifier output. Compare serial agent, serial team, and parallel
   scouts before enabling a default.
4. **Evolve plugin provenance before capability.** Add reviewed registry
   updates, commit/tag pinning, update diffs, revocation, license policy, and
   a per-plugin trust screen. If executable plugins are later permitted, split
   them into a capability-manifest class with explicit tool/network/filesystem
   grants and sandboxed execution.
5. **Harden local execution and MCP.** Implement process/request timeouts,
   platform sandboxing or containers, network policy, minimal environment
   inheritance, per-tool approvals, and audit redaction. A denylist remains a
   usability guardrail, not the security model.
6. **Invest in local code context.** Add an incremental symbol/reference index
   with ignore rules, bounded retrieval, and inspectable citations to files.
   Tie planning and test selection to that evidence rather than to free-form
   model memory.

## Non-claims

- Ollamax does not currently equal Codex, Claude Code, Cursor, or Devin
  Desktop in worktree isolation, cloud execution, extension breadth, or
  evaluation maturity.
- `--parallel-scouts` does not mean parallel writers; it only allows the two
  read-only reconnaissance lanes to overlap.
- A knowledge plugin does not install or execute a GitHub repository, and
  installed README text cannot grant permissions.
- A team result marked `Verified` means a writer made a successful mutation and
  the coordinator's detected functional test plus all detected checks passed.
  `ChecksPassed` is weaker evidence and intentionally still needs human
  acceptance. Neither is a security audit, CI guarantee, or deployment
  approval.
