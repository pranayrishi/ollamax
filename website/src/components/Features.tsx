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
    title: "A companion that sees your screen",
    body: "Press a hotkey, speak, and a small overlay companion answers out loud — it reads your screen with a local vision model, points at the exact button or menu it means, and never sends a pixel off your machine. Whisper for ears, your Ollama model for brains, free system voices for speech.",
    icon: "🎙",
  },
  {
    title: "Circle anything, then ask",
    body: "Draw a quick circle around any region of your screen — a search bar, a chart, a layout you like — and tell the companion what to do with it. Ask it to explain, or say “replicate this in my project” and it hands a ready-to-run task, screenshot attached, to the coding agent.",
    icon: "◯",
  },
  {
    title: "The 2026 open-model lineup, curated",
    body: "Qwen 3.6, Gemma 4, DeepSeek R1 and V3.1, MiniMax M2.5, Qwen3-VL vision models — hardware-aware selection picks what actually fits your RAM/VRAM, and heterogeneous teams put reasoning models on planning and coder models on writing, in parallel.",
    icon: "⚙",
  },
  {
    title: "Private by design",
    body: "A built-in secret scanner catches keys before they reach a model, per-project memory and chat history stay on your device, an optional replay log makes sessions reproducible, and inference runs locally. Open source, MIT-licensed.",
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
