// Kept as a reusable status panel for pages that need a compact package callout.
// It deliberately does not point at `releases/latest`: the source-tree feature
// set is not represented by the older public v0.2.0 assets.
import Link from "next/link";

const primary = [
  { os: "macOS", sub: "Apple Silicon" },
  { os: "Windows", sub: "x64" },
  { os: "Linux", sub: "x64" },
];

export function DownloadButtons({ compact = false }: { compact?: boolean }) {
  return (
    <div>
      <div className={compact ? "flex flex-wrap gap-3" : "grid gap-3 sm:grid-cols-3"}>
        {primary.map((t) => (
          <Link
            key={t.os}
            href="/download"
            className="surface-subtle flex items-center justify-between gap-3 px-4 py-3 text-sm transition-colors hover:bg-secondary"
          >
            <span className="font-medium text-foreground">Package status · {t.os}</span>
            <span className="text-xs text-muted-foreground">{t.sub}</span>
          </Link>
        ))}
      </div>
      <p className="mt-4 text-xs leading-relaxed text-muted-foreground">
        Replacement packages are withheld until their matching public release and checksums are verified. See the{" "}
        <Link href="/download" className="text-link">download status →</Link>.
      </p>
    </div>
  );
}
