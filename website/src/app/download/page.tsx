import type { Metadata } from "next";
import { Nav } from "@/components/Nav";
import { Footer } from "@/components/Footer";
import { DownloadGrid } from "@/components/DownloadGrid";

export const metadata: Metadata = { title: "Download" };

export default function DownloadPage() {
  return (
    <>
      <Nav />
      <main id="main" className="mx-auto max-w-3xl px-4 py-16">
        <h1 className="text-3xl font-bold tracking-tight text-zinc-50">Download Ollama-Forge</h1>
        <p className="mt-3 text-zinc-400">
          Free and open source. Each download is a <strong className="text-zinc-200">quick-setup
          bundle</strong> — the <code>forge</code> engine + the VS Code chat/agent/build panel + a
          one-step install script. It&rsquo;s <em>not</em> a one-click app yet; setup takes about two
          minutes.
        </p>

        {/* Honest, prominent disclosures — unsigned + prerequisite. */}
        <div className="mt-6 space-y-3">
          <div className="rounded-xl border border-amber-500/30 bg-amber-500/5 p-4 text-sm text-amber-200/90">
            <strong className="text-amber-200">Unsigned build.</strong> Your OS will warn you the
            first time (we haven&rsquo;t paid for code signing yet). It&rsquo;s safe to run:
            <ul className="mt-2 list-disc space-y-1 pl-5 text-amber-200/80">
              <li><strong>macOS:</strong> right-click <code>forge</code> (or the app) → <strong>Open</strong>, then confirm. The bundled <code>install.sh</code> also clears the quarantine flag.</li>
              <li><strong>Windows:</strong> if SmartScreen appears, click <strong>More info → Run anyway</strong>.</li>
            </ul>
          </div>
          <div className="rounded-xl border border-ink-700 bg-ink-900/60 p-4 text-sm text-zinc-400">
            <strong className="text-zinc-200">Requires Ollama.</strong> The app runs models locally
            via Ollama. Install it from{" "}
            <a href="https://ollama.com/download" className="text-ember-400 hover:underline" target="_blank" rel="noopener noreferrer">ollama.com/download</a>{" "}
            and pull a model (e.g. <code>ollama pull qwen2.5-coder:7b</code>). The bundle&rsquo;s
            install script checks for it.
          </div>
        </div>

        <div className="mt-8">
          <DownloadGrid />
        </div>

        <p className="mt-8 text-sm text-zinc-500">
          After downloading, unpack and run the included <code>install.sh</code> (macOS/Linux) or{" "}
          <code>install.ps1</code> (Windows) — see <code>README-FIRST.md</code> inside. Verify any
          download against its SHA-256 checksum link above.
        </p>
      </main>
      <Footer />
    </>
  );
}
