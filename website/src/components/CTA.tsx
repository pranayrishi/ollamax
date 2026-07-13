import Link from "next/link";
import { DesktopAppDownload } from "./DesktopAppDownload";
import { SignupCounter } from "./SignupCounter";

export function CTA() {
  return (
    <section id="download" className="scroll-mt-24">
      <div className="mx-auto max-w-3xl px-6 py-24 sm:px-8 sm:py-32">
        <div className="surface p-6 sm:p-10">
          <h2 className="text-center font-display text-4xl leading-[0.98] tracking-[-0.04em] text-foreground sm:text-5xl">
            Get Ollamax
          </h2>
          <p className="mx-auto mt-5 max-w-xl text-center leading-relaxed text-muted-foreground">
            Free and open source. Requires a local Ollama daemon. Sign in with GitHub only when you
            want account features — your code, prompts, and files still never leave your machine.
          </p>

          <div className="mt-8">
            <DesktopAppDownload />
          </div>

          <SignupCounter />

          <p className="mt-5 text-center text-sm text-muted-foreground">
            Want it inside your own editor, or need checksums and all platforms?{" "}
            <Link href="/download" className="text-link">See all download options →</Link>
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
