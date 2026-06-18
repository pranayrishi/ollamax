import { SectionHeading } from "./SectionHeading";

const steps = [
  {
    n: "1",
    title: "Install Ollama + the app",
    body: "Download the desktop app and have a local Ollama running. The app detects your hardware and recommends a model that fits.",
  },
  {
    n: "2",
    title: "Sign in with GitHub (optional)",
    body: "One GitHub identity across the app and this site. Sign-in is for your account — it never gates local use, and your code never touches our servers.",
  },
  {
    n: "3",
    title: "Build, chat, and ship — locally",
    body: "Open the side panel, pick a model, and work. Inference runs on your machine via Ollama; nothing is proxied through us.",
  },
];

export function HowItWorks() {
  return (
    <section id="how" className="scroll-mt-20 border-y border-ink-700/60 bg-ink-900/40">
      <div className="mx-auto max-w-6xl px-4 py-20">
        <SectionHeading
          eyebrow="How it works"
          title="Up and running in three steps"
        />
        <div className="mt-12 grid gap-6 md:grid-cols-3">
          {steps.map((s) => (
            <div key={s.n} className="relative rounded-2xl border border-ink-700 bg-ink-900/70 p-6">
              <div className="mb-4 grid h-9 w-9 place-items-center rounded-full border border-ember-500/40 bg-ember-500/10 font-semibold text-ember-400">
                {s.n}
              </div>
              <h3 className="mb-2 font-semibold text-zinc-100">{s.title}</h3>
              <p className="text-sm leading-relaxed text-zinc-400">{s.body}</p>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}
