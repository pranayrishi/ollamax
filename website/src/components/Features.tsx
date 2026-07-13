import { SectionHeading } from "./SectionHeading";

const features = [
  {
    title: "Chat + an autonomous Agent",
    body: "Ask mode for quick, read-only answers about your code. Agent mode plans, uses tools, edits files, and runs tasks end-to-end — both in one Cursor-style side panel.",
    icon: "◧",
  },
  {
    title: "Safe file edits, always previewed",
    body: "The agent writes and refactors real files, but every change is shown as a diff you approve first — with path-traversal guards so it can only touch your workspace. Nothing is applied behind your back.",
    icon: "✎",
  },
  {
    title: "Voice navigation, on-device",
    body: "Press a key, speak, and jump straight to the code you mean. Speech-to-text runs locally with a bundled whisper.cpp model — your audio never leaves the machine.",
    icon: "🎙",
  },
  {
    title: "Per-project memory",
    body: "Each project keeps its own context and chat history on your device, so the assistant remembers what you're working on without ever syncing your code to a server.",
    icon: "◉",
  },
  {
    title: "Hardware-aware model selection",
    body: "Detects your RAM/VRAM and recommends an Ollama model that actually fits, with context-window and capability hints. Pick any installed model from the picker.",
    icon: "⚙",
  },
  {
    title: "Private by design",
    body: "A built-in secret scanner catches keys before they reach a model, an optional replay log makes sessions reproducible, and inference runs locally. Open source, MIT-licensed.",
    icon: "🛡",
  },
];

export function Features() {
  return (
    <section id="studio" className="mx-auto max-w-7xl scroll-mt-24 px-6 py-24 sm:px-8 sm:py-32">
      <span id="features" className="relative -top-24 block" aria-hidden="true" />
      <SectionHeading
        eyebrow="The Ollamax studio"
        title="A real coding assistant, held quietly on your own machine."
        subtitle="Everything a cloud assistant gives you, without your code ever leaving your hardware."
      />
      <div className="mt-12 grid gap-5 sm:grid-cols-2 lg:grid-cols-3">
        {features.map((f) => (
          <article
            key={f.title}
            className="group surface-subtle p-6 transition-colors hover:bg-secondary"
          >
            <div className="liquid-glass mb-5 grid h-11 w-11 place-items-center rounded-2xl text-lg text-foreground">
              {f.icon}
            </div>
            <h3 className="mb-3 text-2xl leading-none tracking-[-0.02em] text-foreground">{f.title}</h3>
            <p className="text-sm leading-relaxed text-muted-foreground">{f.body}</p>
          </article>
        ))}
      </div>
    </section>
  );
}
