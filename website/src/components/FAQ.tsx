import { SectionHeading } from "./SectionHeading";

// Native <details>/<summary> accordion — fully keyboard-accessible with zero JS.
const faqs = [
  {
    q: "Do I need an account to use the app?",
    a: "No. Local inference works fully signed-out — the local-first promise is intact. Signing in with GitHub establishes your identity across the app and this site and unlocks future account features; it never gates basic functionality.",
  },
  {
    q: "Why GitHub-only sign-in?",
    a: "One identity, no passwords for us to store, and it's the account developers already have. There is no email/password and no other provider, by design.",
  },
  {
    q: "Does my code get sent to your servers?",
    a: "No. In the local-first configuration, inference runs through local Ollama or an explicitly configured loopback self-hosted server. We never receive prompts, code, file contents, paths, or repo names. Ollamax has no hosted-provider integration or automatic cloud fallback; the backend handles accounts, the Hub catalog, downloads, and anonymous usage metadata only.",
  },
  {
    q: "What about telemetry / the usage dashboard?",
    a: "We collect anonymous usage metadata — counts of messages/builds, which model and provider, token counts, and language inferred from file extensions — to power your personal usage dashboard on the website. It contains no content. There's a telemetry toggle in the app; turn it off and nothing is sent. You can export or delete your data anytime.",
  },
  {
    q: "How do I sign in?",
    a: "Sign in with GitHub — that's your single account across the website and the desktop app. (Other sign-in options may come later.)",
  },
  {
    q: "Is it really one account across web and desktop?",
    a: "Yes. Both the website login and the desktop app's 'Sign in with GitHub' resolve to the same GitHub identity, so it's a single account — the same model as Cursor and Windsurf.",
  },
  {
    q: "Is it open source?",
    a: "Yes, MIT-licensed. You can read every line, fork it, and run it offline.",
  },
  {
    q: "What does it cost to run?",
    a: "The local app is free and open source, with no hosted-model billing path. If you operate a server-class local model, its hardware and hosting costs are yours to provision; Ollamax does not run it for you.",
  },
];

export function FAQ() {
  return (
    <section id="faq" className="mx-auto max-w-3xl scroll-mt-24 px-6 py-24 sm:px-8 sm:py-32">
      <SectionHeading eyebrow="A few questions" title="Questions, answered quietly." />
      <div className="surface mt-12 divide-y divide-border overflow-hidden p-0">
        {faqs.map((f) => (
          <details key={f.q} className="group bg-secondary/70 open:bg-muted">
            <summary className="flex cursor-pointer list-none items-center justify-between gap-4 px-5 py-5 text-foreground marker:content-none">
              {f.q}
              <span className="text-muted-foreground transition group-open:rotate-45" aria-hidden="true">
                +
              </span>
            </summary>
            <p className="px-5 pb-5 text-sm leading-relaxed text-muted-foreground">{f.a}</p>
          </details>
        ))}
      </div>
    </section>
  );
}
