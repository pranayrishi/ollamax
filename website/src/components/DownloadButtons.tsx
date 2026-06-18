// Per-OS download buttons. URLs come from NEXT_PUBLIC_DOWNLOAD_* (tied to the
// Phase 3 desktop distribution). When a URL is unset, the button is a disabled
// "coming soon" — we never link to a build that doesn't exist.

const targets = [
  { os: "macOS", env: process.env.NEXT_PUBLIC_DOWNLOAD_MACOS, note: "Apple Silicon · Intel" },
  { os: "Windows", env: process.env.NEXT_PUBLIC_DOWNLOAD_WINDOWS, note: "x64" },
  { os: "Linux", env: process.env.NEXT_PUBLIC_DOWNLOAD_LINUX, note: ".deb · AppImage" },
];

export function DownloadButtons({ compact = false }: { compact?: boolean }) {
  return (
    <div className={compact ? "flex flex-wrap gap-3" : "grid gap-3 sm:grid-cols-3"}>
      {targets.map((t) => {
        const available = !!t.env;
        const base =
          "flex items-center justify-between gap-3 rounded-xl border px-4 py-3 text-sm transition";
        return available ? (
          <a
            key={t.os}
            href={t.env}
            className={`${base} border-ink-600 bg-ink-800 hover:border-ember-500 hover:bg-ink-700`}
          >
            <span className="font-medium text-zinc-100">Download for {t.os}</span>
            <span className="text-xs text-zinc-500">{t.note}</span>
          </a>
        ) : (
          <span
            key={t.os}
            className={`${base} cursor-not-allowed border-ink-700 bg-ink-900 text-zinc-500`}
            aria-disabled="true"
            title="Build URL not configured yet"
          >
            <span className="font-medium">{t.os}</span>
            <span className="text-xs">coming soon</span>
          </span>
        );
      })}
    </div>
  );
}
