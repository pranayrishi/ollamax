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
    a: "No. Inference runs locally on Ollama (or goes directly from your machine to a provider you configure). We never receive prompts, code, file contents, paths, or repo names. The backend handles accounts, the Hub catalog, downloads, and anonymous usage metadata only.",
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
    a: "The local app is free and open source. Cloud models, if you choose to use them, are billed by that provider directly — not by us.",
  },
];

export function FAQ() {
  return (
    <section id="faq" className="mx-auto max-w-3xl scroll-mt-20 px-4 py-20">
      <SectionHeading eyebrow="FAQ" title="Questions, answered" />
      <div className="mt-10 divide-y divide-ink-700/70 overflow-hidden rounded-2xl border border-ink-700">
        {faqs.map((f) => (
          <details key={f.q} className="group bg-ink-900/40 open:bg-ink-900/70">
            <summary className="flex cursor-pointer list-none items-center justify-between gap-4 px-5 py-4 font-medium text-zinc-200 marker:content-none">
              {f.q}
              <span className="text-ember-500 transition group-open:rotate-45" aria-hidden="true">
                +
              </span>
            </summary>
            <p className="px-5 pb-5 text-sm leading-relaxed text-zinc-400">{f.a}</p>
          </details>
        ))}
      </div>
    </section>
  );
}
