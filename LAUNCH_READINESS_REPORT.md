# Launch-Readiness Report: 5 Features + the Honest Same-Day Picture

**Date: 2026-06-18.** You plan to launch to other users **today**, so this leads
with the truth, not optimism. **Bottom line: do NOT launch with enforced login yet
— it's blocked on infrastructure you must stand up first (details in #5).** The
product is otherwise coherent and testable in VS Code now.

---

## Two things that aren't what they looked like (verified, not guessed)

**A. The "Windsurf: Login / Kickbacks" branding is NOT in our app.** I grepped the
whole repo — every "Windsurf" hit is a *comparison* in docs ("like Cursor/Windsurf").
Our extension is installed in **plain VS Code** (`~/.vscode/extensions/`), and the
fork is **Code-OSS (microsoft/vscode)**, rebranded ForgeCode — zero Windsurf. You
have **Windsurf.app installed**; that status bar is *the Windsurf editor itself*.
**Fix: run it in VS Code (where it's installed) or the ForgeCode fork — the Windsurf
UI simply isn't present there.** Nothing to strip from our code.

**B. "Verify in the published artifact" had to shift vehicle.** The fork `.dmg`
rebuild is still **blocked on GitHub Actions billing** (prior round). So the
artifact I verify against today is the **extension installed in your VS Code**,
which I rebuild + reinstall freely (free, seconds). Everything below is verified in
that *installed* artifact, not just the repo.

---

## Feature status (honest)

| # | Feature | Status | Verified in installed artifact? |
|---|---|---|---|
| 5 | **Enforced login** | 🟡 **Code done, infra-blocked** | Gate enforces in code (blocks before any engine call); **can't actually gate without a deployed OAuth server** |
| 2 | **Chat vs Agent, Build removed** | ✅ **Done** | Yes — tabs are `chat`+`agent` only; Build gone; Chat read-only/tools-off, Agent tool-using |
| 1 | **Agentic file-writing** | ⏭️ **Deferred (safety)** | No — not built; would be unsafe to ship to strangers today without the diff/preview rails |
| 3 | **Per-project chat persistence** | ⏭️ **Deferred** | No — follow-up (restore-render needs GUI verification I can't do here) |
| 4 | **Movable right-side chat** | 🟡 **Partial** | Movable = native VS Code (drag to the right side bar); right-*by-default* needs a fork workbench change, not an extension |

---

## 5 — Enforced login (THE launch gate) — what's true

**The code is done and correct.** `_handleSend` calls `_gateBlocks()` and returns
**before** any engine call; on boot `_sendGate()` shows the full-panel sign-in
screen. When `forge.accountServer` is set and the user isn't signed in, **chat and
agent are both blocked.** (Verified `_gateBlocks` present in the installed artifact.)

**Why you still can't launch with it today:** the gate enforces **only when
`forge.accountServer` points at a deployed, working OAuth backend** (your
`website/`). It currently defaults to `""`. So:
- If you leave it blank → no gate (anyone can use it). Not "enforced login."
- If you set it but the website **isn't deployed / OAuth isn't configured** → users
  hit the sign-in screen and **cannot sign in → locked out entirely.** Worse than no
  gate.

**To actually enforce login you must, on your side:**
1. **Deploy `website/`** (Vercel) with **working GitHub + Google OAuth** (client
   IDs/secrets, `AUTH_SECRET`, `DATABASE_URL`) at a real URL.
2. Tell me that URL → I bake it into the extension/fork as the default
   `forge.accountServer` (a `configurationDefaults`), so the gate is **on by default**
   in the shipped build, and I verify a signed-out user is blocked end-to-end.

I will **not** guess a URL or fake this — a wrong URL bricks sign-in for everyone.
**This is the one item between you and a gated launch.**

---

## 2 — Chat vs Agent (Build removed) — done

- **Build tab removed.** Users now see exactly two tabs.
- **Chat = "Ask":** conversational, single-model, **tools OFF, read-only — never
  touches files.** Pure-local (`/api/chat`, `tools:false`).
- **Agent:** autonomous — tools, memory, skills, sub-agent delegation, scheduler,
  the Plan card + per-step approval (`/api/research` → `run_agent_streamed`).
- Tooltips + placeholders rewritten to make the Ask-vs-Agent split obvious.
- **Honest note:** Build's *multi-model orchestration* isn't yet folded *into* Agent
  (Agent runs the single-model tool loop). Removing the tab is shipped; merging the
  orchestrator into Agent is a follow-up.

---

## 1 — Agentic file-writing — deferred, on purpose

You explicitly said: *don't ship an unsafe file-writer to strangers today.* I agree,
so I did **not** rush it. The safe design (next focused pass), reusing the existing
`extract_and_write_code_blocks` (which already has path-traversal guards):
1. Agent emits fenced code blocks tagged with a target path.
2. Host extracts + path-guards each (reject absolute / `..` / outside-workspace).
3. **Show a VS Code diff (`vscode.diff`) with Apply / Discard** per file (or
   auto-apply with a one-click Undo via a `WorkspaceEdit`), open applied files.
4. Only **Agent** writes; **Chat** stays read-only.

This is ~a day of careful, GUI-verified work. Shipping it half-tested to real users
risks their files — not worth it for the same-day launch, and not required for it.

---

## 3 / 4 — persistence + right-side dock

- **#3 per-project memory:** the on-device memory layer already persists session
  summaries; full **chat-history restore keyed by workspace** is a clean follow-up
  (the risk is rendering restored history correctly, which I can't GUI-verify here).
- **#4 movable chat:** dragging the panel between the left and right side bars is
  **already native VS Code** — you can move it now. Making it **default to the right**
  needs a workbench-layout default baked into the **ForgeCode fork** (not something an
  extension can force). So: movable ✅ today; right-by-default = fork work.

---

## What is genuinely launch-ready *today*

- ✅ A **coherent two-tab product** (Chat = safe/read-only, Agent = autonomous) in
  VS Code, with the full local engine (memory, skills, tools, plan/approval, vision).
- ✅ Runs **100% local** on the user's Ollama — no cloud, no keys, no telemetry by
  default.
- ✅ The **login gate works the moment** you point it at a live account server.

## What must happen before launching to strangers
1. **Stand up the OAuth website + give me the URL** → real enforced login (#5).
2. **Tell users to run it in VS Code**, not Windsurf (or wait for the signed
   ForgeCode fork once Actions billing is resolved).
3. **File-writing (#1)** ships in the next pass with diff/preview — not today.

## Open questions
1. Deployed **account-server URL** (the launch gate)?
2. Do you want me to build **#1 file-writing (with diff/preview)** next, or **#3
   persistence**, first?
3. Distribute as the **VS Code extension** (works now) or hold for the **ForgeCode
   fork** download (needs Actions billing)?
