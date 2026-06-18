# Build Report: Hermes-Class Agent + Voice Demo Navigation

**Date: 2026-06-18.** Follows the signed-off [HERMES_VOICE_DESIGN.md](HERMES_VOICE_DESIGN.md)
(Phase 0). Sign-off decisions implemented: **Hybrid/reimplement (no Python Hermes)**,
**core agent + NL scheduler**, **include the sandboxed shell**, **base.en STT
lazy-downloaded**. Backend model-efficiency (MLX/MoE/quant) remained out of scope.

## TL;DR

Two flagship features were built on the local models, reusing forge's primitives:
1. **A Hermes-class autonomous Agent** — persistent memory write-back, skills in the
   loop, sub-agent delegation, an expanded sandboxed tool set (file + shell), a
   **native MCP client**, and an **on-device NL scheduler** — plus the **3-tab
   reframe** that makes Chat/Agent/Build genuinely distinct, and a legible Agent
   activity timeline.
2. **Voice demo navigation** — push-to-talk → on-device whisper.cpp STT → intent→code
   over the graph → editor reveal, with a status-bar state machine and Undo/Next.

**Verification: 187 Rust tests pass (was 166 — +21 new); all changed extension JS
passes `node --check`; the extension packages to a valid `.vsix`.** Shipped across
small, individually-tested commits.

---

## What was researched (Phase 0)

A 5-agent workflow + adversarial synthesis **fetched the real Hermes sources**
(`docs/llms-full.txt` HTTP 200, the GitHub repo, `agentskills.io`) and read forge's
code. Verified-from-source: Hermes is **MIT, Python/uv, local-first, a full MCP
client, uses `agentskills.io` SKILL.md**, with persistent memory + self-authored
skills + sub-agent delegation + one NL cron + a messaging gateway. **Flagged as
unverifiable** (third-party mirror only, and self-contradictory): exact tool/toolset
counts, worker-pool size, the 64k floor — so the build targets the *verified* pillars,
not those numbers. Full analysis + the integrate-vs-reimplement reasoning is in the
design doc.

---

## What was built — native vs integrated, mapped to existing primitives

**Decision: reimplement on forge's primitives + adopt the open standards Hermes is
built on. No Python Hermes engine.** (Rationale in the design doc: a live Python
process every turn would break single-binary packaging; the graphify "bundle Python"
precedent doesn't transfer because that's build-time-only.)

| Capability | How | Built on (existing primitive) |
|---|---|---|
| **Persistent memory** | `run_agent_streamed` now **writes a session summary at stream end** (`summarize_session`→`remember`), SecurityGuard-gated, on-device. Closes the "nothing wrote memory" gap. | `src/memory/` |
| **Self-improving skills** | New `SkillsEngine::best_match` **auto-applies the most relevant skill** into the agent prompt + emits a `skill_applied` event. Adopts the `agentskills.io` `SKILL.md` standard already parsed. | `src/skills/` |
| **Sub-agent delegation** | New `delegate` **tool** spawns an isolated child `Agent` with a fresh context + restricted read-only toolset; only its answer returns. Recursion prevented structurally. | `src/agent/`, `src/tools/`, `src/executor/` |
| **Expanded tools** | New **sandboxed file tools** (`fs_read/write/edit`, path-traversal-proof) + a **sandboxed shell** (deny-list + timeout kill-switch + on-device audit + `FORGE_SHELL_DISABLED`). | `src/tools/` |
| **MCP** (the integrate piece) | New **native MCP client** (`src/mcp/`): JSON-RPC-over-stdio, `initialize`→`tools/list`→`tools/call`, each remote tool wrapped as a forge `Tool` (`mcp__<server>__<tool>`), behind an **allowlist**. This is the one capability "pulled toward Hermes" — reversing the graphify no-MCP stance, since here **MCP is the product surface**. | `src/tools/` `ToolRegistry` |
| **Scheduling** | New `src/scheduler/`: NL parsing ("every day at 9:30", "in 2 hours"), persistent JSONL store, and a **background tick in `forge serve`** that runs due tasks via a non-streaming agent pass. | new, fully on-device |
| **3-tab reframe** | **Chat** = `/api/chat` tools **off** (pure-local quick Q&A). **Agent** = `/api/research` (the upgraded `run_agent_streamed` — all of the above). **Build** = Orchestrator, unchanged. | re-pointed endpoints |
| **Voice nav** | `CodeGraph::locate` + `GET /api/voice/locate` (intent→`{file,line,symbol}`); extension `forge.voiceNavigate` captures mic→16 kHz WAV→**local whisper.cpp**→locate→reveal. | `src/graph/`, `src/hub/` scoring |

**Deferred (recommended in the design, explicitly): the 20+ messaging-platform
connectors** — wrong shape for a desktop IDE.

---

## UI decisions

- **3 distinct tabs** with rewritten tooltips (talk / act-autonomously / build).
- **Agent Activity Timeline** (legible & trustworthy): per-step status dot, tool +
  result preview, and an **expandable `args`** disclosure (the agent's args were
  captured but never shown). New rows surface **skills-in-the-loop** (✦) and
  **recalled memory** (⌘). All on the existing token system + CSP (no new color
  world, no inline handlers).
- **Voice**: a push-to-talk panel with a **mic-orb state machine**
  (idle/listening/transcribing/locating), live "Heard: … → file:line", **Undo/Next**,
  and a status-bar item. Designed first-timer-legible per the UI research.

**Autonomy Dial — now real (follow-on wave).** The agent **pauses before
consequential tools** (`fs_write`/`fs_edit`/`shell`) and asks the user. Engine:
an `ApprovalPolicy` the agent consults before each consequential tool (Deny → the
tool is skipped, not executed); a `ChannelApprovalPolicy` with three modes —
**auto** (allow all), **confirm** (emit `approval_request`, await the user, 120 s
timeout → deny), **readonly** (deny all consequential). `POST /api/agent/approve`
relays the decision to the waiting run. Webview: an **Autonomy Dial** select
(default *Confirm each*, shown only in Agent mode) + an inline **Approve/Deny**
prompt in the timeline. This is genuine step-level intervention.

**Still deferred UI**: the **Plan card / Intent Preview** (the agent emitting a
plan before executing) and a separate **sub-agent lane** (the `delegate` tool
currently discards child steps rather than streaming them to a lane). Pause/Resume
is partially covered by per-step Deny + the existing Stop.

---

## Privacy / safety posture (unchanged guarantees)

- **Local-first**: memory, skills, scheduler, voice STT all on-device. Voice audio
  never reaches the engine — only the transcript text hits `/api/voice/locate`. No
  Web Speech API.
- **Memory write-back is SecurityGuard-gated** — nothing that scans as a secret is
  persisted.
- **Shell** ships deny-list + timeout + audit log + kill-switch; **file tools** are
  sandboxed to the workspace; **MCP** is allowlisted. The new tools widen the trust
  surface, so **interactive per-call consent (the Autonomy Dial) now gates them** —
  in *confirm* mode the agent pauses for Approve/Deny before any write/shell.

---

## Verification

- `cargo test`: **187 passed / 0 failed** (+21 new: files 5, shell 4, skills 1,
  delegate 1, scheduler 5, mcp 4, graph-locate 1 — counts approximate per module).
- All changed extension JS: `node --check` clean; extension packages to a valid
  `.vsix` (17 files).
- Commits (no attribution): `ffe3b83` file/shell, `bce8376` memory/skills, `c0727ba`
  delegate, `1b85e91` scheduler, `53db863` MCP, `fda822f` reframe+timeline, `f45b7ac`
  voice-locate (engine), `33afa3b` voice (extension).

---

## Open questions / risks (honest)

1. **Live STT is unvalidated here** — no microphone or whisper model in this
   environment. The deterministic **intent→code** core is unit-tested; the
   capture→whisper.cpp→reveal pipeline is built + syntax-clean but needs whisper.cpp
   installed + a real mic to exercise end-to-end. STT accuracy on code identifiers
   ("verify_token") and base.en latency depend on the **separately-handled
   model-efficiency work**.
2. **MCP transport is unexercised** — no MCP server available here; config parsing,
   JSON-RPC framing, and tool mapping are tested; the live stdio handshake is not.
3. **Voice nav quality = graph coverage** — needs the project indexed
   (`graphify-out/graph.json`); embeddings remain deferred (lexical recall first).
4. **Agent latency** — a cold 7B first token (~21 s per the perf report) makes future
   per-step approval feel slow; mitigation is to confirm only consequential actions.
   Improves with your model-efficiency work.
5. **New tools widen the trust surface** — interactive consent (Autonomy Dial) +
   Pause/Resume + sub-agent lane are the deferred UI follow-up.
6. **Built on the forge-vscode extension** (the carried-over AI layer for the Code-OSS
   pivot) — verified-packageable; the fork itself still needs a real build machine.
7. **DailyAt schedules use the host's local timezone** via chrono; documented.
