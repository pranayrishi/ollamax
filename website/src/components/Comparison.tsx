import { SectionHeading } from "./SectionHeading";

// Honest comparison. Claims are about *our* product's design, not benchmarks
// or competitor disparagement. "Cloud-only assistant" is a category, not a
// named product, to avoid unverifiable claims about specific tools.
const rows: { feature: string; forge: boolean | string; cloud: boolean | string }[] = [
  { feature: "Runs fully offline (local models)", forge: true, cloud: false },
  { feature: "Your code stays on your machine", forge: true, cloud: "Sent to provider" },
  { feature: "Agent edits files behind a diff you approve", forge: true, cloud: "Varies" },
  { feature: "On-device voice navigation (audio stays local)", forge: true, cloud: false },
  { feature: "Hardware-aware model selection", forge: true, cloud: "n/a" },
  { feature: "Built-in secret scanner before send", forge: true, cloud: false },
  { feature: "Reproducible local replay/audit log", forge: true, cloud: false },
  { feature: "Telemetry is anonymous + opt-out (no content)", forge: true, cloud: "Often collects usage + content" },
  { feature: "Open source (MIT)", forge: true, cloud: "Usually no" },
];

function Cell({ v }: { v: boolean | string }) {
  if (v === true) return <span className="text-ember-400">✓</span>;
  if (v === false) return <span className="text-zinc-600">—</span>;
  return <span className="text-xs text-zinc-400">{v}</span>;
}

export function Comparison() {
  return (
    <section className="mx-auto max-w-5xl px-4 py-20">
      <SectionHeading eyebrow="Why local-first" title="A different trade-off" />
      <div className="mt-10 overflow-hidden rounded-2xl border border-ink-700">
        <table className="w-full text-left text-sm">
          <thead className="bg-ink-800 text-zinc-300">
            <tr>
              <th scope="col" className="px-5 py-4 font-medium">Capability</th>
              <th scope="col" className="px-5 py-4 text-center font-semibold text-ember-400">
                Ollamax
              </th>
              <th scope="col" className="px-5 py-4 text-center font-medium text-zinc-400">
                Typical cloud-only assistant
              </th>
            </tr>
          </thead>
          <tbody className="divide-y divide-ink-700/70">
            {rows.map((r) => (
              <tr key={r.feature} className="bg-ink-900/40">
                <td className="px-5 py-3.5 text-zinc-300">{r.feature}</td>
                <td className="px-5 py-3.5 text-center">
                  <Cell v={r.forge} />
                </td>
                <td className="px-5 py-3.5 text-center">
                  <Cell v={r.cloud} />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  );
}
