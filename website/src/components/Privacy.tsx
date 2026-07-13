import { SectionHeading } from "./SectionHeading";
import Link from "next/link";

export function Privacy() {
  return (
    <section id="privacy" className="scroll-mt-24 border-y border-border bg-secondary/65">
      <div className="mx-auto max-w-5xl px-6 py-24 sm:px-8 sm:py-32">
        <SectionHeading
          eyebrow="Privacy"
          title="Your code stays where your work does."
          subtitle="We collect anonymous usage metadata (counts, not content) that you can turn off. We never receive your prompts, code, or files."
        />
        <div className="mt-12 grid gap-5 md:grid-cols-2">
          <article className="surface p-6">
            <h3 className="mb-3 text-2xl leading-none tracking-[-0.02em] text-foreground">Inference never touches our servers</h3>
            <p className="text-sm leading-relaxed text-muted-foreground">
              Prompts, code, and model responses run on your machine via local Ollama, or go directly
              from your machine to a provider you choose. Our backend never receives, proxies, logs,
              or stores any of it. There is literally no table in our database for it.
            </p>
          </article>
          <article className="surface-subtle p-6">
            <h3 className="mb-3 text-2xl leading-none tracking-[-0.02em] text-foreground">What we store</h3>
            <ul className="space-y-2 text-sm text-muted-foreground">
              <li>· Your GitHub identity (id, name, avatar, email if granted)</li>
              <li>· <strong>Usage metadata</strong>: counts of messages/builds, which model/provider, token counts, and language by file extension</li>
              <li>· <strong>Never</strong>: prompt text, code, file contents, file paths, or repo names</li>
              <li>· You can turn telemetry off and delete your data anytime</li>
            </ul>
          </article>
        </div>
        <p className="mt-7 text-center text-sm text-muted-foreground">
          Full details in the{" "}
          <Link href="/privacy" className="text-link">
            privacy note
          </Link>
          .
        </p>
      </div>
    </section>
  );
}
