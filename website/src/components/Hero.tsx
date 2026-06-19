import { signInGitHub } from "@/app/actions";
import { GitHubMark } from "./GitHubMark";
// FUTURE: Google sign-in disabled for now — see src/auth.ts.
// import { GoogleMark } from "./GoogleMark";
import { SignupCounter } from "./SignupCounter";

const badges = [
  "Local-first",
  "Bring your own models",
  "Open source (MIT)",
  "GitHub sign-in",
];

export function Hero() {
  return (
    <section className="relative overflow-hidden">
      <div className="glow pointer-events-none absolute inset-x-0 top-0 h-[480px]" aria-hidden="true" />
      <div className="mx-auto max-w-6xl px-4 pb-20 pt-20 text-center sm:pt-28">
        <p className="mb-5 inline-flex items-center gap-2 rounded-full border border-ink-600 bg-ink-900/70 px-4 py-1.5 text-xs text-zinc-400">
          <span className="h-1.5 w-1.5 rounded-full bg-ember-500" />
          Your code stays on your machine
        </p>
        <h1 className="mx-auto max-w-3xl text-balance text-4xl font-bold tracking-tight text-zinc-50 sm:text-6xl">
          Local-first AI coding.{" "}
          <span className="bg-gradient-to-r from-ember-400 to-ember-600 bg-clip-text text-transparent">
            Your models, your machine.
          </span>
        </h1>
        <p className="mx-auto mt-6 max-w-2xl text-pretty text-lg text-zinc-400">
          Chat, a tool-using research agent, and parallel multi-model builds — running on your own
          hardware through local Ollama. Hardware-aware model selection, a built-in secret scanner,
          and a reproducible audit trail. No cloud required.
        </p>

        <div className="mt-9 flex flex-col items-center justify-center gap-3 sm:flex-row">
          <a
            href="#download"
            className="w-full rounded-xl bg-ember-500 px-6 py-3 font-semibold text-ink-950 hover:bg-ember-400 sm:w-auto"
          >
            Download the app
          </a>
          <form action={signInGitHub} className="w-full sm:w-auto">
            <button
              type="submit"
              className="flex w-full items-center justify-center gap-2 rounded-xl border border-ink-600 bg-ink-800 px-6 py-3 font-semibold text-zinc-100 hover:border-ember-500 sm:w-auto"
            >
              <GitHubMark className="h-5 w-5" />
              Sign in with GitHub
            </button>
          </form>
        </div>

        <ul className="mt-10 flex flex-wrap items-center justify-center gap-x-6 gap-y-2 text-sm text-zinc-500">
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
      </div>
    </section>
  );
}
