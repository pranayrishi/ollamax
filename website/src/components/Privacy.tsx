import { SectionHeading } from "./SectionHeading";
import Link from "next/link";

export function Privacy() {
  return (
    <section id="privacy" className="scroll-mt-20 border-y border-ink-700/60 bg-ink-900/40">
      <div className="mx-auto max-w-5xl px-4 py-20">
        <SectionHeading
          eyebrow="Privacy"
          title="Your code stays on your machine."
          subtitle="We collect anonymous usage metadata (counts, not content) that you can turn off. We never receive your prompts, code, or files."
        />
        <div className="mt-12 grid gap-5 md:grid-cols-2">
          <div className="rounded-2xl border border-ember-500/30 bg-ember-500/[0.06] p-6">
            <h3 className="mb-3 font-semibold text-zinc-100">Inference never touches our servers</h3>
            <p className="text-sm leading-relaxed text-zinc-400">
              Prompts, code, and model responses run on your machine via local Ollama, or go directly
              from your machine to a provider you choose. Our backend never receives, proxies, logs,
              or stores any of it. There is literally no table in our database for it.
            </p>
          </div>
          <div className="rounded-2xl border border-ink-700 bg-ink-900/70 p-6">
            <h3 className="mb-3 font-semibold text-zinc-100">What we store</h3>
            <ul className="space-y-2 text-sm text-zinc-400">
              <li>· Your GitHub identity (id, name, avatar, email if granted)</li>
              <li>· <strong>Usage metadata</strong>: counts of messages/builds, which model/provider, token counts, and language by file extension</li>
              <li>· <strong>Never</strong>: prompt text, code, file contents, file paths, or repo names</li>
              <li>· You can turn telemetry off and delete your data anytime</li>
            </ul>
          </div>
        </div>
        <p className="mt-6 text-center text-sm text-zinc-500">
          Full details in the{" "}
          <Link href="/privacy" className="text-ember-400 underline underline-offset-4">
            privacy note
          </Link>
          .
        </p>
      </div>
    </section>
  );
}
