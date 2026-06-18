import { DownloadButtons } from "./DownloadButtons";
import { signInGitHub } from "@/app/actions";
import { GitHubMark } from "./GitHubMark";

export function CTA() {
  return (
    <section id="download" className="scroll-mt-20">
      <div className="mx-auto max-w-5xl px-4 py-20">
        <div className="relative overflow-hidden rounded-3xl border border-ink-700 bg-ink-900 p-8 text-center sm:p-12">
          <div className="glow pointer-events-none absolute inset-x-0 top-0 h-40" aria-hidden="true" />
          <h2 className="text-3xl font-bold tracking-tight text-zinc-50 sm:text-4xl">
            Download Ollama-Forge
          </h2>
          <p className="mx-auto mt-3 max-w-xl text-zinc-400">
            Free and open source. Requires a local Ollama daemon. Sign in with GitHub to link your
            account — optional, and never required for local use.
          </p>

          <div className="mx-auto mt-8 max-w-2xl">
            <DownloadButtons />
          </div>

          <div className="mt-6 flex items-center justify-center">
            <form action={signInGitHub}>
              <button
                type="submit"
                className="flex items-center gap-2 rounded-xl border border-ink-600 bg-ink-800 px-5 py-2.5 text-sm font-semibold text-zinc-100 hover:border-ember-500"
              >
                <GitHubMark className="h-4 w-4" />
                Sign in with GitHub
              </button>
            </form>
          </div>

          {/*
            NOTE: The reference site shows testimonials, star ratings, and
            download counters here. Those are intentionally OMITTED — we do not
            ship fabricated social proof. Add a real testimonials section with
            verifiable quotes/attribution when you have them.
          */}
        </div>
      </div>
    </section>
  );
}
