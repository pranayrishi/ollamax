import { signInGitHub } from "@/app/actions";
import { GitHubMark } from "./GitHubMark";
// FUTURE: Google sign-in disabled for now — see src/auth.ts.
// import { GoogleMark } from "./GoogleMark";
import { SignupCounter } from "./SignupCounter";

const badges = ["Local-first", "Bring your own models", "Open source (MIT)", "On-device voice"];

export function Hero() {
  return (
    <section className="relative overflow-hidden">
      <div className="glow pointer-events-none absolute inset-x-0 top-0 h-[520px]" aria-hidden="true" />
      <div className="mx-auto max-w-6xl px-4 pb-16 pt-20 text-center sm:pt-28">
        <p className="mb-5 inline-flex items-center gap-2 rounded-full border border-ink-600 bg-ink-900/70 px-4 py-1.5 text-xs text-zinc-400">
          <span className="h-1.5 w-1.5 rounded-full bg-ember-500" />
          Your code never leaves your machine
        </p>
        <h1 className="mx-auto max-w-3xl text-balance text-4xl font-bold tracking-tight text-zinc-50 sm:text-6xl">
          AI coding that runs{" "}
          <span className="bg-gradient-to-r from-ember-400 to-ember-600 bg-clip-text text-transparent">
            on your hardware.
          </span>
        </h1>
        <p className="mx-auto mt-6 max-w-2xl text-pretty text-lg text-zinc-400">
          Ollamax brings chat, an autonomous agent that edits your files safely, and on-device voice
          navigation to your editor — powered by local Ollama. No cloud, no API keys, no code
          leaving your machine.
        </p>

        <div className="mt-9 flex flex-col items-center justify-center gap-3 sm:flex-row">
          <a
            href="#download"
            className="w-full rounded-xl bg-ember-500 px-6 py-3 font-semibold text-ink-950 transition hover:bg-ember-400 sm:w-auto"
          >
            Download Ollamax
          </a>
          <form action={signInGitHub} className="w-full sm:w-auto">
            <button
              type="submit"
              className="flex w-full items-center justify-center gap-2 rounded-xl border border-ink-600 bg-ink-800 px-6 py-3 font-semibold text-zinc-100 transition hover:border-ember-500 sm:w-auto"
            >
              <GitHubMark className="h-5 w-5" />
              Sign in with GitHub
            </button>
          </form>
        </div>

        <ul className="mt-8 flex flex-wrap items-center justify-center gap-x-6 gap-y-2 text-sm text-zinc-500">
          {badges.map((b) => (
            <li key={b} className="flex items-center gap-2">
              <span className="text-ember-500" aria-hidden="true">
                ✓
              </span>
              {b}
            </li>
          ))}
        </ul>

        <SignupCounter />

        <ProductMockup />
      </div>
    </section>
  );
}

/** A static, styled preview of the Ollamax side panel — chat + a safe-edit diff + voice. */
function ProductMockup() {
  return (
    <div className="mx-auto mt-14 max-w-3xl text-left" aria-hidden="true">
      <div className="overflow-hidden rounded-2xl border border-ink-700 bg-ink-900/80 shadow-2xl shadow-black/40 backdrop-blur">
        {/* title bar */}
        <div className="flex items-center gap-2 border-b border-ink-700/70 bg-ink-900 px-4 py-3">
          <span className="h-3 w-3 rounded-full bg-[#ff5f57]" />
          <span className="h-3 w-3 rounded-full bg-[#febc2e]" />
          <span className="h-3 w-3 rounded-full bg-[#28c840]" />
          <span className="ml-3 text-xs text-zinc-500">Ollamax — auth-service</span>
          <span className="ml-auto flex gap-1 text-[11px]">
            <span className="rounded-md px-2 py-1 text-zinc-500">Ask</span>
            <span className="rounded-md bg-ember-500/15 px-2 py-1 font-medium text-ember-300">Agent</span>
          </span>
        </div>

        {/* conversation */}
        <div className="space-y-4 p-5">
          <div className="flex justify-end">
            <p className="max-w-[80%] rounded-2xl rounded-br-sm bg-ember-500/15 px-4 py-2 text-sm text-zinc-100">
              Add rate limiting to the login route and write a test.
            </p>
          </div>

          <div className="flex justify-start">
            <div className="max-w-[88%] space-y-3">
              <p className="rounded-2xl rounded-bl-sm bg-ink-800 px-4 py-2 text-sm text-zinc-300">
                I&rsquo;ll add a token-bucket limiter to <code className="text-ember-300">login.ts</code> and
                cover it with a test. Review the diff:
              </p>

              {/* diff card */}
              <div className="overflow-hidden rounded-xl border border-ink-700 bg-ink-950/60 font-mono text-[12px] leading-relaxed">
                <div className="flex items-center justify-between border-b border-ink-700/70 px-3 py-2 text-zinc-500">
                  <span>src/routes/login.ts</span>
                  <span className="text-[10px] uppercase tracking-wider text-ember-400">diff</span>
                </div>
                <div className="px-3 py-2">
                  <div className="text-zinc-500">@@ login handler @@</div>
                  <div className="rounded bg-emerald-500/10 text-emerald-300">+ const ok = await limiter.take(ip);</div>
                  <div className="rounded bg-emerald-500/10 text-emerald-300">+ if (!ok) return res.status(429);</div>
                  <div className="text-zinc-400">&nbsp;&nbsp;return authenticate(req, res);</div>
                </div>
                <div className="flex gap-2 border-t border-ink-700/70 px-3 py-2">
                  <span className="rounded-md bg-ember-500 px-3 py-1 text-[11px] font-semibold text-ink-950">
                    Apply
                  </span>
                  <span className="rounded-md border border-ink-600 px-3 py-1 text-[11px] text-zinc-400">
                    Discard
                  </span>
                </div>
              </div>
            </div>
          </div>

          {/* voice chip */}
          <div className="flex items-center gap-2 pt-1 text-xs text-zinc-500">
            <span className="inline-flex items-center gap-1.5 rounded-full border border-ink-600 bg-ink-800 px-3 py-1">
              <span className="text-ember-400">🎙</span> Listening — &ldquo;go to the rate limiter&rdquo;
            </span>
            <span className="text-zinc-600">· on-device · running llama3 locally</span>
          </div>
        </div>
      </div>
    </div>
  );
}
