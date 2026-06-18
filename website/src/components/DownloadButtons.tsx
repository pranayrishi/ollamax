// Per-OS download buttons. URLs come from NEXT_PUBLIC_DOWNLOAD_* (set by the
// release pipeline once signed installers are published). Until then we DON'T
// link to a build that doesn't exist — instead the button points at the GitHub
// Releases page (where the signed builds will appear), so it's never a dead
// no-op. NOTE: NEXT_PUBLIC_* values are inlined at BUILD time — set them in
// Vercel and REDEPLOY for them to take effect.

const repo = process.env.NEXT_PUBLIC_GITHUB_REPO;
const releasesUrl = repo ? `${repo.replace(/\/$/, "")}/releases` : null;

const targets = [
  { os: "macOS", env: process.env.NEXT_PUBLIC_DOWNLOAD_MACOS, note: "Apple Silicon · Intel" },
  { os: "Windows", env: process.env.NEXT_PUBLIC_DOWNLOAD_WINDOWS, note: "x64" },
  { os: "Linux", env: process.env.NEXT_PUBLIC_DOWNLOAD_LINUX, note: ".deb · AppImage" },
];

export function DownloadButtons({ compact = false }: { compact?: boolean }) {
  return (
    <div className={compact ? "flex flex-wrap gap-3" : "grid gap-3 sm:grid-cols-3"}>
      {targets.map((t) => {
        const direct = !!t.env;
        const href = t.env || releasesUrl;
        const base =
          "flex items-center justify-between gap-3 rounded-xl border px-4 py-3 text-sm transition";

        // No artifact URL AND no repo to fall back to → honest disabled state.
        if (!href) {
          return (
            <span
              key={t.os}
              className={`${base} cursor-not-allowed border-ink-700 bg-ink-900 text-zinc-500`}
              aria-disabled="true"
              title="Build not published yet"
            >
              <span className="font-medium">{t.os}</span>
              <span className="text-xs">coming soon</span>
            </span>
          );
        }

        return (
          <a
            key={t.os}
            href={href}
            {...(direct ? {} : { target: "_blank", rel: "noopener noreferrer" })}
            className={`${base} border-ink-600 bg-ink-800 hover:border-ember-500 hover:bg-ink-700`}
          >
            <span className="font-medium text-zinc-100">
              {direct ? `Download for ${t.os}` : t.os}
            </span>
            <span className="text-xs text-zinc-500">{direct ? t.note : "view releases ↗"}</span>
          </a>
        );
      })}
    </div>
  );
}
