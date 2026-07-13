import type { Metadata } from "next";
import { Nav } from "@/components/Nav";
import { Footer } from "@/components/Footer";

export const metadata: Metadata = {
  title: "Privacy",
  description: "What Ollamax collects, what it never touches, and how to control it.",
};

export default function PrivacyPage() {
  return (
    <>
      <Nav />
      <main id="main" className="page-frame max-w-3xl">
        <h1 className="page-title">Privacy note</h1>
        <p className="mt-4 text-sm text-muted-foreground">
          Plain-language. This is a draft pending legal review — the precise list of what&rsquo;s
          collected is below.
        </p>

        <div className="mt-12 space-y-6 text-foreground/85">
          <section className="surface">
            <h2 className="text-3xl leading-none tracking-[-0.02em] text-foreground">The one-sentence version</h2>
            <p className="mt-4 leading-relaxed text-muted-foreground">
              <strong className="text-foreground">Your code stays on your machine.</strong> Inference
              runs locally (Ollama) or goes directly from your machine to a provider you chose — never
              through us. We collect <strong className="text-foreground">anonymous usage metadata</strong>{" "}
              (counts and categories, no content) to power your usage dashboard, and{" "}
              <strong className="text-foreground">you can turn it off</strong>.
            </p>
          </section>

          <section className="surface">
            <h2 className="text-3xl leading-none tracking-[-0.02em] text-foreground">What our backend stores</h2>
            <p className="mt-4 text-muted-foreground">Account identity:</p>
            <ul className="mt-3 list-disc space-y-1 pl-5 text-muted-foreground">
              <li>Your GitHub account id (the stable key)</li>
              <li>Name, avatar URL, and email (email only if granted/verified)</li>
              <li>Which providers you&rsquo;ve linked, and sign-in timestamps</li>
            </ul>
            <p className="mt-5 text-muted-foreground">Usage metadata (only with telemetry on):</p>
            <ul className="mt-3 list-disc space-y-1 pl-5 text-muted-foreground">
              <li>Event counts and timestamps for chat / agent / build / Hub activations</li>
              <li>Which model and provider were used, and token counts</li>
              <li>Programming language <em>inferred from a file extension</em> (e.g. &ldquo;rust&rdquo;)</li>
              <li>Auto-routing decisions; suggestions made vs. accepted (counts only)</li>
            </ul>
          </section>

          <section className="surface">
            <h2 className="text-3xl leading-none tracking-[-0.02em] text-foreground">What we NEVER collect</h2>
            <ul className="mt-4 list-disc space-y-1 pl-5 text-muted-foreground">
              <li>Prompt text, chat messages, or conversations</li>
              <li>Generated code or your source code</li>
              <li>File contents, full file paths, or directory structure</li>
              <li>Repository names or URLs</li>
              <li>Any inference traffic</li>
            </ul>
            <p className="mt-4 leading-relaxed text-muted-foreground">
              The analytics endpoint validates every event server-side and{" "}
              <strong className="text-foreground">rejects anything content-shaped</strong> — an unknown
              field, an over-long string, or a string with whitespace. So content can&rsquo;t be stored
              even by a misbehaving client.
            </p>
          </section>

          <section className="surface">
            <h2 className="text-3xl leading-none tracking-[-0.02em] text-foreground">Your controls</h2>
            <ul className="mt-4 list-disc space-y-1 pl-5 text-muted-foreground">
              <li><strong>Telemetry toggle</strong> in the app (Settings → Ollamax → Telemetry). Off = nothing is sent.</li>
              <li><strong>Pause</strong> collection and <strong>delete</strong> all your usage data from the web dashboard.</li>
              <li><strong>Export</strong> your usage metadata as JSON.</li>
              <li>Account deletion removes your identity and all linked data.{" "}
                <span className="text-muted-foreground">[Owner: wire a self-serve account-delete + a contact address.]</span>
              </li>
            </ul>
          </section>

          <section className="surface">
            <h2 className="text-3xl leading-none tracking-[-0.02em] text-foreground">Sign-in &amp; tokens</h2>
            <p className="mt-4 leading-relaxed text-muted-foreground">
              Sign in with GitHub (no passwords). The desktop app stores its session token in
              your OS keychain. We never see your GitHub client secret or raw provider token —
              the app exchanges a PKCE-protected code for our own short-lived token. The elevated
              GitHub permission used for the optional &ldquo;support maintainers&rdquo; starring is
              requested only at that moment and the resulting token is never stored.
            </p>
          </section>
        </div>
      </main>
      <Footer />
    </>
  );
}
