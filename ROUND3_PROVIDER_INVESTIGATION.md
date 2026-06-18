# Round 3 — Provider Investigation & Decision Request

> **Status: paused for one decision before writing any provider code.** Per your
> own instruction ("confirm with me what each repo represents… don't bluff
> provider integrations"), I verified what the four repos actually are before
> touching `src/providers/`. They are **not** the official-API SDKs Feature 1
> describes, and wiring three of them in as-is would break the privacy/ToS
> guarantees you told me to preserve. Findings + options + the decision I need
> are below. Generated 2026-06-17.

---

## TL;DR

- I fan-out-investigated all four repos (read their READMEs/source first-hand).
- **None is an official OpenAI/Anthropic/Gemini client.** Three are
  free/unofficial **access** projects; one is just a docs catalog.
- The request's framing — *official wire formats, API keys in the OS keychain,
  cloud labeled, secret-scanner as a hard gate* — assumes **bring-your-own
  official key**. The repos' center of gravity is the **opposite**: getting paid
  models **without** a sanctioned key (reverse-engineering, subscription-token
  reuse, closed proxies).
- "Store the user's API key in the keychain" is **incoherent** for those flows
  (no key, or a borrowed token, or an aggregator key that hands your code to a
  third party). And routing a user's source through those hops is **worse for
  privacy than calling the official cloud APIs directly** — fatal to the
  finance/healthcare/defense compliance pitch.
- **Recommendation: implement Feature 1 as you literally described it** (Option
  A below) — native Rust providers behind the existing `LlmProvider` trait,
  official wire formats, keys in the OS keychain, secret-scan block before cloud
  egress — and use the four repos **only as design reference, porting zero
  transport code.** I need you to confirm this (or redirect) before I build.

---

## What each repo actually is (verified, first-hand)

| Repo | What it really is | Access model | License | Verdict for forge |
| :-- | :-- | :-- | :-- | :-- |
| **xtekky/gpt4free** | GPLv3 Python aggregator; **keyless access to frontier models via reverse-engineered / proxied web endpoints**. OpenAI sent a takedown-or-lawsuit letter in 2023; ships a `LEGAL_NOTICE.md` offloading ToS compliance to users. | No keys; scraped/proxied 3rd-party endpoints | **GPLv3** (copyleft — incompatible with forge's MIT) | **Reject as transport.** ToS-violating, worse privacy, license contamination. Reference only. |
| **diegosouzapw/OmniRoute** | Self-hosted TS gateway (one OpenAI-compatible endpoint → 160–227 providers). Legit with your own keys, **but its headline "free Claude/GPT/Gemini" reuses subscription-IDE tokens (Amazon Q/Kiro, Alibaba/Qoder) behind TLS-fingerprint stealth.** | Mixed: BYOK **+** subscription-token reuse | MIT | **Reject the "free" path** (high ToS risk, anti-detection ≠ a ToS-clean brand). BYOK-only mode is OK but adds an external moving part. |
| **Gitlawb/openclaude** | TS fork of Anthropic's Claude Code → multi-provider agent CLI. Supports BYOK official providers, **but fresh-install default routes through the author's closed proxy** (`opengateway.gitlawb.com`), **stores keys in plaintext JSON**, and ships a **gray-zone ChatGPT-OAuth "Codex" path**. | Mixed: BYOK + closed proxy default + OAuth | MIT (own changes) **+ carries Anthropic Claude Code code & "Claude" trademark** | **Reject defaults.** Useful as a per-provider **config-schema** reference. Don't copy its plaintext keys or proxy default. |
| **ShaikhWarsi/free-ai-tools** | **Not software** — a Markdown + Next.js **catalog** of AI providers/tools. No client, no proxy, no auth code. Some pricing is forward-dated/speculative (`[verify]` tags). | N/A (catalog) | MIT | **Use as a provider shortlist** only (re-verify all pricing/limits). No code to port. |

**Collective picture:** three runnable aggregator/router/fork projects built
around free/unofficial/proxied access (two TS, one GPLv3 Python) + one docs list.
A set of *multi-provider routing references* — **not** an official-API SDK to
drive an integration.

## Why this collides with the product (and your own brief)

- Your brief: "**The moment a user picks a cloud model, their code leaves their
  machine** … preserve 'the UI can't phone home' … **API keys are secrets:
  store them in the OS keychain** … be honest about local vs cloud." All of that
  assumes **official, contractual, per-vendor access**.
- gpt4free/OmniRoute-free/openclaude-default instead send your prompts and source
  through **opaque, contract-less intermediaries** (community mirrors, the
  gitlawb proxy, resold Amazon/Alibaba backends) that can log/retain/train on
  them. That is *more* exposed than a direct official API call, and it makes the
  README's "no other outbound connections, data stays on your machine" claim
  **false**.
- One instance of forge caught scraping or token-spoofing destroys the
  "auditable, zero-cloud, ToS-clean" value proposition regardless of legal
  outcome.

---

## Options

**A. Official paid APIs, user keys in the keychain, thin first-party Rust
transport — RECOMMENDED.**
Implement exactly what the request describes: `OpenAiProvider`,
`AnthropicProvider`, `GeminiProvider` as siblings behind the existing
`LlmProvider` trait in [src/providers/](src/providers/), each speaking the real
official wire format (OpenAI chat-completions SSE deltas, Anthropic Messages
stream events, Gemini `streamGenerateContent`), normalized to the **existing**
`forge serve` SSE contract (`meta → token* → done`) so the extension/webview
barely change. Keys via the OS keychain (`keyring` crate) — never plaintext,
never logged, never in the replay log, never in the webview. Secret scanner stays
a **hard pre-flight gate** before any byte leaves the machine. Cloud models
clearly labeled. Repos used as research only (borrow openclaude's per-provider
config schema + free-ai-tools' shortlist); **zero transport code ported.**
- *Pros:* preserves every stated guarantee; MIT-clean; drops straight into
  `LlmProvider`/`ProviderPool`; auditable; each format is a few hundred lines.
- *Cons:* I write/maintain three streaming parsers + keychain handling; **no
  "free" models** — users bring & pay for their own keys (the honest cost of the
  posture).

**B. One OpenAI-compatible client (base-url + key switching) + native Anthropic/
Gemini adapters.** Covers OpenAI, OpenRouter, Groq, Cerebras, DeepSeek, Mistral,
Fireworks, LM Studio, Ollama's `/v1` with one client; add native adapters for
Anthropic/Gemini (their formats aren't OpenAI-compatible). Layer this *under* A
to cut code. BYOK only; never enable any "free"/token-reuse mode.

**C. Catalog/metadata only.** Mine the repos for *which* providers to support +
their official endpoints/limits; build transport via A/B; ship none of their
running code. (A natural precursor to A.)

**D. Integrate the free/unofficial repos as-is — NOT RECOMMENDED.** Gives users
"free" GPT/Claude/Gemini, but directly violates the official-API+keychain design
and the local-first/ToS-clean brand; high legal exposure (OpenAI already issued a
gpt4free takedown); GPLv3 + Anthropic-code contamination; worse privacy;
unstable endpoints. Listed only for completeness.

**Recommended: A, folded with B (to cut code) and C (repos as research).**

---

## The honest tradeoff to state plainly

There is **no compliant "free frontier models" path.** Every "free" mechanism in
these repos is precisely what a privacy-/ToS-clean tool must not do. So Option A
means **users bring and pay for their own keys.** If a free-frontier-models
capability is actually the goal, that's a *different product* with a different
risk posture — and I should hear that from you explicitly rather than infer it.

---

## What I'll do once you confirm (defaults I'm proposing)

- **Secret-scan gate:** for **cloud** sends, **block on Critical/High** with an
  explicit per-message override (your Round-3 decision); **local Ollama stays
  warn-only** (nothing leaves the machine).
- **Keys:** OS keychain via the `keyring` crate; redacted everywhere (logs,
  replay, webview, errors).
- **Privacy invariants kept:** webview `connect-src 'none'` stays; *all* provider
  calls (local + cloud) go through the extension host / `forge serve`, never the
  webview; honest local-vs-cloud labeling in CLI + panel.
- **Model registry:** data-driven (config the repo's `tools.ts`-style data can
  seed), **live-fetched** where a provider exposes a list endpoint + a key is
  present, falling back to the registry offline/unkeyed. **No hardcoded list.**
- **Features 2 (no-limits + queue) and 3 (status labels)** are fully independent
  of the provider question and unblocked.

See the questions I'm asking alongside this document for the decision.
