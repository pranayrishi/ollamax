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
    title: "Voice, lasso, + a private cursor cue",
    body: "Hold to speak only when a local whisper.cpp runtime is configured; a package may not include one. A small click-through cue reports local voice or selection state without seeing transcripts or pixels. A lasso sends only a bounded crop to a local vision request; screen-derived briefs are not kept in memory or replay, and neither speech nor vision falls back to a hosted API.",
    icon: "⌁",
  },
  {
    title: "Per-project memory",
    body: "Each project keeps its own context and chat history on your device, so the assistant remembers what you're working on without ever syncing your code to a server.",
    icon: "◉",
  },
  {
    title: "Hardware-aware local model routing",
    body: "Ollama is the default path, with current local Qwen, Gemma 4, and DeepSeek recommendations. Advanced users can explicitly select a configured loopback self-hosted endpoint in Chat, Agent, Research, and Team; DeepSeek V4 and MiniMax M3 remain server-class deployments, not automatic downloads or cloud fallbacks.",
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
        subtitle="Local inference, local visual context, and explicit approval for workspace changes."
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
