// Homepage per-OS download buttons → real bundle assets on the public releases
// repo (see src/lib/downloads.ts). Each download is a CLI + VS Code extension
// bundle with a quick setup (labelled honestly), not a one-click app. macOS
// defaults to Apple Silicon; Intel + checksums live on the /download page.
import Link from "next/link";
import { assetUrl } from "@/lib/downloads";

const primary = [
  { os: "macOS", sub: "Apple Silicon", asset: "ollama-forge-macos-arm64.tar.gz" },
  { os: "Windows", sub: "x64", asset: "ollama-forge-windows-x64.zip" },
  { os: "Linux", sub: "x64", asset: "ollama-forge-linux-x64.tar.gz" },
];

export function DownloadButtons({ compact = false }: { compact?: boolean }) {
  return (
    <div>
      <div className={compact ? "flex flex-wrap gap-3" : "grid gap-3 sm:grid-cols-3"}>
        {primary.map((t) => (
          <a
            key={t.os}
            href={assetUrl(t.asset)}
            className="flex items-center justify-between gap-3 rounded-xl border border-ink-600 bg-ink-800 px-4 py-3 text-sm transition hover:border-ember-500 hover:bg-ink-700"
          >
            <span className="font-medium text-zinc-100">Download for {t.os}</span>
            <span className="text-xs text-zinc-500">{t.sub}</span>
          </a>
        ))}
      </div>
      <p className="mt-3 text-xs text-zinc-500">
        These are direct (unsigned) downloads — there&rsquo;s a{" "}
        <Link href="/download#first-launch" className="text-ember-400 hover:underline">one-time step to open it →</Link>.
        For zero warnings, use the{" "}
        <Link href="/download" className="text-ember-400 hover:underline">one-line installer</Link>{" "}
        · needs{" "}
        <a href="https://ollama.com/download" className="text-zinc-400 hover:text-ember-400" target="_blank" rel="noopener noreferrer">Ollama</a>.
      </p>
    </div>
  );
}
