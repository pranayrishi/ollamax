import { SectionHeading } from "./SectionHeading";

const steps = [
  {
    n: "1",
    title: "Install Ollama + Ollamax",
    body: "Download the app and have a local Ollama running. Ollamax detects your hardware and recommends a model that actually fits your machine.",
  },
  {
    n: "2",
    title: "Sign in with GitHub",
    body: "One quick sign-in links your account across the app and this site. It's only your identity — your code, prompts, and files never touch our servers.",
  },
  {
    n: "3",
    title: "Chat, edit, and navigate by voice",
    body: "Open the side panel, pick a model, and work. Ask questions, let the agent edit files (you approve each diff), or jump around your code by voice — all running locally via Ollama.",
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
