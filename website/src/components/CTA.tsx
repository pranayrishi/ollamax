import Link from "next/link";
import { DesktopAppDownload } from "./DesktopAppDownload";

export function CTA() {
  return (
    <section id="download" className="scroll-mt-20">
      <div className="mx-auto max-w-3xl px-4 py-20">
        <div className="relative overflow-hidden rounded-3xl border border-ink-700 bg-ink-900 p-6 sm:p-10">
          <div className="glow pointer-events-none absolute inset-x-0 top-0 h-40" aria-hidden="true" />
          <h2 className="text-center text-3xl font-bold tracking-tight text-zinc-50 sm:text-4xl">
            Get Ollamax
          </h2>
          <p className="mx-auto mt-3 max-w-xl text-center text-zinc-400">
            Free and open source. Requires a local Ollama daemon. Sign in once with GitHub to get
            started — your code, prompts, and files still never leave your machine.
          </p>

          <div className="mt-8">
            <DesktopAppDownload />
          </div>

          <p className="mt-5 text-center text-sm text-zinc-500">
            Want it inside your own editor, or need checksums and all platforms?{" "}
            <Link href="/download" className="text-ember-400 hover:underline">See all download options →</Link>
          </p>

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
