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
        <h1 className="page-title">Your usage dashboard</h1>
        <p className="page-lede">Sign in to see your usage.</p>
        <form action={signInGitHub} className="mt-6">
          <button className="button-primary gap-2">
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
        <h1 className="page-title">Your usage</h1>
        <span className="text-xs text-muted-foreground">Web-only · your data only · metadata, never content</span>
      </div>

      {usage.totals.events === 0 ? (
        <p className="surface mt-10 text-muted-foreground">
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
                  className="flex-1 rounded-sm bg-foreground/70"
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
              <p className="text-sm text-foreground/85">
                {usage.suggestions.made} made · {usage.suggestions.accepted} accepted
                {pctAi != null ? ` (${pctAi}% accepted)` : ""}
              </p>
            </Panel>
          </div>
        </>
      )}

      {/* Telemetry controls */}
      <div className="surface mt-10">
        <h2 className="eyebrow">Your data &amp; telemetry</h2>
        <p className="mt-4 text-sm leading-relaxed text-muted-foreground">
          We collect <strong>anonymous usage metadata</strong> (counts, models, languages by file
          extension) to power this dashboard. <strong>Your code stays on your machine</strong> — no
          prompt text, code, file contents, paths, or repo names are ever sent. Status:{" "}
          <strong className={optedOut ? "text-amber-300" : "text-foreground"}>
            {optedOut ? "paused" : "collecting"}
          </strong>
          .
        </p>
        <div className="mt-5 flex flex-wrap gap-3">
          <a
            href="/api/analytics/export"
            className="button-secondary px-5"
          >
            Export my data
          </a>
          {optedOut ? (
            <form action={setTelemetry.bind(null, false)}>
              <button className="button-secondary px-5">
                Resume collection
              </button>
            </form>
          ) : (
            <form action={setTelemetry.bind(null, true)}>
              <button className="button-secondary px-5">
                Pause collection
              </button>
            </form>
          )}
          <form action={deleteMyUsage}>
            <button className="button-secondary px-5 text-red-300 hover:border-red-500/60">
              Delete my usage data
            </button>
          </form>
        </div>
        <p className="mt-4 text-xs text-muted-foreground">
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
      <main id="main" className="page-frame max-w-4xl">
        {children}
      </main>
      <Footer />
    </>
  );
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className="surface p-5">
      <div className="text-2xl font-medium text-foreground">{value}</div>
      <div className="mt-1 text-xs text-muted-foreground">{label}</div>
    </div>
  );
}

function Panel({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="surface mt-6">
      <h3 className="mb-5 text-2xl leading-none tracking-[-0.02em] text-foreground">{title}</h3>
      {children}
    </section>
  );
}

function Bars({ rows }: { rows: { label: string; n: number }[] }) {
  if (rows.length === 0) return <p className="text-sm text-muted-foreground">No data.</p>;
  const max = Math.max(1, ...rows.map((r) => r.n));
  return (
    <div className="space-y-2">
      {rows.map((r) => (
        <div key={r.label} className="text-xs">
          <div className="mb-1 flex justify-between">
            <span className="truncate text-foreground/85">{r.label}</span>
            <span className="text-muted-foreground">{r.n}</span>
          </div>
          <div className="h-2 rounded-full bg-muted">
            <div className="h-2 rounded-full bg-foreground/70" style={{ width: `${(r.n / max) * 100}%` }} />
          </div>
        </div>
      ))}
    </div>
  );
}
