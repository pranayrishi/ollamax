import type { Metadata } from "next";
import { auth } from "@/auth";
import { getUserUsage, getUserById } from "@/lib/db";
import { Nav } from "@/components/Nav";
import { Footer } from "@/components/Footer";
import { signInGitHub } from "@/app/actions";
import { GitHubMark } from "@/components/GitHubMark";
import { deleteMyUsage, setTelemetry } from "./actions";

export const metadata: Metadata = { title: "Usage dashboard" };
export const dynamic = "force-dynamic";

export default async function Dashboard() {
  const session = await auth();
  const userId = session?.user?.accountId;

  if (!userId) {
    return (
      <Shell>
        <h1 className="text-3xl font-bold text-zinc-50">Your usage dashboard</h1>
        <p className="mt-2 text-zinc-400">Sign in to see your usage.</p>
        <form action={signInGitHub} className="mt-6">
          <button className="flex items-center gap-2 rounded-xl bg-ember-500 px-5 py-2.5 font-semibold text-ink-950 hover:bg-ember-400">
            <GitHubMark className="h-4 w-4" /> Sign in
          </button>
        </form>
      </Shell>
    );
  }

  const [usage, user] = await Promise.all([getUserUsage(userId), getUserById(userId)]);
  const optedOut = !!user?.telemetry_opt_out;
  const pctAi =
    usage.suggestions.made > 0
      ? Math.round((usage.suggestions.accepted / usage.suggestions.made) * 100)
      : null;
  const dailyMax = Math.max(1, ...usage.daily.map((d) => d.n));

  return (
    <Shell>
      <div className="flex items-center justify-between">
        <h1 className="text-3xl font-bold text-zinc-50">Your usage</h1>
        <span className="text-xs text-zinc-500">Web-only · your data only · metadata, never content</span>
      </div>

      {usage.totals.events === 0 ? (
        <p className="mt-8 rounded-2xl border border-ink-700 bg-ink-900/60 p-6 text-zinc-400">
          No usage recorded yet.{" "}
          {optedOut
            ? "Telemetry is paused — resume it below to start collecting your own metadata."
            : "Use the app (with telemetry enabled) and your activity will show up here."}
        </p>
      ) : (
        <>
          <div className="mt-8 grid gap-4 sm:grid-cols-4">
            <Stat label="Total events" value={usage.totals.events.toLocaleString()} />
            <Stat label="Tokens in" value={usage.totals.tokensIn.toLocaleString()} />
            <Stat label="Tokens out" value={usage.totals.tokensOut.toLocaleString()} />
            <Stat label="% AI-assisted" value={pctAi == null ? "—" : `${pctAi}%`} />
          </div>

          <Panel title="Activity (last 90 days)">
            <div className="flex items-end gap-1" style={{ height: 80 }}>
              {usage.daily.map((d) => (
                <div
                  key={d.day}
                  title={`${d.day}: ${d.n}`}
                  className="flex-1 rounded-sm bg-ember-500/70"
                  style={{ height: `${Math.max(4, (d.n / dailyMax) * 100)}%` }}
                />
              ))}
            </div>
          </Panel>

          <div className="grid gap-5 md:grid-cols-2">
            <Panel title="Feature usage">
              <Bars rows={usage.byType.map((r) => ({ label: r.type, n: r.n }))} />
            </Panel>
            <Panel title="Models / providers">
              <Bars rows={usage.byModel.map((r) => ({ label: `${r.model}${r.provider ? ` · ${r.provider}` : ""}`, n: r.n }))} />
            </Panel>
            <Panel title="Languages">
              <Bars rows={usage.byLanguage.map((r) => ({ label: r.language, n: r.n }))} />
            </Panel>
            <Panel title="Suggestions">
              <p className="text-sm text-zinc-300">
                {usage.suggestions.made} made · {usage.suggestions.accepted} accepted
                {pctAi != null ? ` (${pctAi}% accepted)` : ""}
              </p>
            </Panel>
          </div>
        </>
      )}

      {/* Telemetry controls */}
      <div className="mt-10 rounded-2xl border border-ink-700 bg-ink-900/60 p-6">
        <h2 className="text-sm font-semibold uppercase tracking-widest text-zinc-500">Your data &amp; telemetry</h2>
        <p className="mt-3 text-sm text-zinc-400">
          We collect <strong>anonymous usage metadata</strong> (counts, models, languages by file
          extension) to power this dashboard. <strong>Your code stays on your machine</strong> — no
          prompt text, code, file contents, paths, or repo names are ever sent. Status:{" "}
          <strong className={optedOut ? "text-amber-300" : "text-ember-400"}>
            {optedOut ? "paused" : "collecting"}
          </strong>
          .
        </p>
        <div className="mt-5 flex flex-wrap gap-3">
          <a
            href="/api/analytics/export"
            className="rounded-xl border border-ink-600 bg-ink-800 px-4 py-2 text-sm text-zinc-200 hover:border-ember-500"
          >
            Export my data
          </a>
          {optedOut ? (
            <form action={setTelemetry.bind(null, false)}>
              <button className="rounded-xl border border-ink-600 bg-ink-800 px-4 py-2 text-sm text-zinc-200 hover:border-ember-500">
                Resume collection
              </button>
            </form>
          ) : (
            <form action={setTelemetry.bind(null, true)}>
              <button className="rounded-xl border border-ink-600 bg-ink-800 px-4 py-2 text-sm text-zinc-200 hover:border-ember-500">
                Pause collection
              </button>
            </form>
          )}
          <form action={deleteMyUsage}>
            <button className="rounded-xl border border-ink-600 bg-ink-800 px-4 py-2 text-sm text-red-300 hover:border-red-500/60">
              Delete my usage data
            </button>
          </form>
        </div>
        <p className="mt-3 text-xs text-zinc-500">
          You can also toggle telemetry in the app (Settings → Ollamax → Telemetry).
        </p>
      </div>
    </Shell>
  );
}

function Shell({ children }: { children: React.ReactNode }) {
  return (
    <>
      <Nav />
      <main id="main" className="mx-auto max-w-4xl px-4 py-16">
        {children}
      </main>
      <Footer />
    </>
  );
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-2xl border border-ink-700 bg-ink-900/60 p-5">
      <div className="text-2xl font-bold text-zinc-50">{value}</div>
      <div className="mt-1 text-xs text-zinc-500">{label}</div>
    </div>
  );
}

function Panel({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="mt-6 rounded-2xl border border-ink-700 bg-ink-900/60 p-6">
      <h3 className="mb-4 text-sm font-semibold text-zinc-300">{title}</h3>
      {children}
    </section>
  );
}

function Bars({ rows }: { rows: { label: string; n: number }[] }) {
  if (rows.length === 0) return <p className="text-sm text-zinc-500">No data.</p>;
  const max = Math.max(1, ...rows.map((r) => r.n));
  return (
    <div className="space-y-2">
      {rows.map((r) => (
        <div key={r.label} className="text-xs">
          <div className="mb-1 flex justify-between">
            <span className="truncate text-zinc-300">{r.label}</span>
            <span className="text-zinc-500">{r.n}</span>
          </div>
          <div className="h-2 rounded-full bg-ink-700">
            <div className="h-2 rounded-full bg-ember-500/70" style={{ width: `${(r.n / max) * 100}%` }} />
          </div>
        </div>
      ))}
    </div>
  );
}
