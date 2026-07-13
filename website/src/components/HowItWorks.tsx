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
    <section id="how" className="scroll-mt-24 border-y border-border bg-secondary/65">
      <div className="mx-auto max-w-7xl px-6 py-24 sm:px-8 sm:py-32">
        <SectionHeading
          eyebrow="A calmer way in"
          title="Up and running in three considered steps."
        />
        <div className="mt-12 grid gap-6 md:grid-cols-3">
          {steps.map((s) => (
            <article key={s.n} className="surface-subtle relative p-6 sm:p-7">
              <div className="liquid-glass mb-5 grid h-10 w-10 place-items-center rounded-full text-sm font-medium text-foreground">
                {s.n}
              </div>
              <h3 className="mb-3 text-2xl leading-none tracking-[-0.02em] text-foreground">{s.title}</h3>
              <p className="text-sm leading-relaxed text-muted-foreground">{s.body}</p>
            </article>
          ))}
        </div>
      </div>
    </section>
  );
}
