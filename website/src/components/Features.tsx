import { SectionHeading } from "./SectionHeading";

const features = [
  {
    title: "Three modes, one panel",
    body: "Chat for quick edits, a tool-using research Agent (web · wiki · arXiv), and Build — parallel workers across different models at once. A Cursor-style side panel in your editor.",
    icon: "◧",
  },
  {
    title: "Hardware-aware model selection",
    body: "Detects your RAM/VRAM and recommends a model that actually fits, with per-model context-window and capability hints. Pick any installed Ollama model from the picker.",
    icon: "⚙",
  },
  {
    title: "Built-in secret scanner",
    body: "Scans attached files for API keys, private keys, and tokens before anything is sent to a model — so credentials don't leak into a prompt by accident.",
    icon: "🛡",
  },
  {
    title: "Reproducible audit trail",
    body: "An optional replay log records the model digest, seed, and a SHA-256 of every prompt and response, so a past session can be re-run and verified bit-for-bit.",
    icon: "🧾",
  },
  {
    title: "Graceful long conversations",
    body: "Real token-budgeting trims the oldest context to fit the model's window — visibly, never silently — and a message queue lets you line up follow-ups while a reply streams.",
    icon: "∞",
  },
  {
    title: "Bring your own models",
    body: "Runs entirely on local Ollama by default. Open source and MIT-licensed — read it, fork it, run it offline.",
    icon: "⧉",
  },
];

export function Features() {
  return (
    <section id="features" className="mx-auto max-w-6xl scroll-mt-20 px-4 py-20">
      <SectionHeading
        eyebrow="Features"
        title="Everything you need, running locally"
        subtitle="A harness that makes local models genuinely useful for real coding work."
      />
      <div className="mt-12 grid gap-5 sm:grid-cols-2 lg:grid-cols-3">
        {features.map((f) => (
          <div
            key={f.title}
            className="rounded-2xl border border-ink-700 bg-ink-900/60 p-6 transition hover:border-ink-600"
          >
            <div className="mb-4 grid h-10 w-10 place-items-center rounded-xl bg-ink-700 text-lg">
              {f.icon}
            </div>
            <h3 className="mb-2 font-semibold text-zinc-100">{f.title}</h3>
            <p className="text-sm leading-relaxed text-zinc-400">{f.body}</p>
          </div>
        ))}
      </div>
    </section>
  );
}
