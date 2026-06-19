import type { Metadata } from "next";
import { Nav } from "@/components/Nav";
import { Footer } from "@/components/Footer";
import { DownloadGrid } from "@/components/DownloadGrid";
import { DesktopAppDownload } from "@/components/DesktopAppDownload";
import { CopyCommand } from "@/components/CopyCommand";
import { FirstLaunchGuide } from "@/components/FirstLaunchGuide";

export const metadata: Metadata = { title: "Download" };

const RELEASES_REPO =
  process.env.NEXT_PUBLIC_RELEASES_REPO || "https://github.com/pranayrishi/ollamax-releases";
const SH_URL = `${RELEASES_REPO.replace(/\/$/, "")}/releases/latest/download/install.sh`;
const PS_URL = `${RELEASES_REPO.replace(/\/$/, "")}/releases/latest/download/install.ps1`;

export default function DownloadPage() {
  return (
    <>
      <Nav />
      <main id="main" className="mx-auto max-w-3xl px-4 py-16">
        <h1 className="text-3xl font-bold tracking-tight text-zinc-50">Download Ollamax</h1>
        <p className="mt-3 text-zinc-400">
          Free and open source. Get the <strong className="text-zinc-200">Ollamax desktop app</strong>{" "}
          below — engine, on-device voice, and sign-in built in — or use the same experience inside
          your own editor with the one-line installer.
        </p>

        {/* PRIMARY: the standalone desktop app */}
        <div className="mt-8">
          <DesktopAppDownload />
        </div>

        {/* SECONDARY: use it in your own editor (no security warning). */}
        <section id="one-line" className="mt-8 scroll-mt-24 rounded-2xl border border-ink-700 bg-ink-900/40 p-6">
          <h2 className="text-sm font-semibold uppercase tracking-widest text-zinc-400">
            Or · use it in your own editor (no warning)
          </h2>
          <div className="mt-4 space-y-4">
            <CopyCommand label="macOS / Linux" command={`curl -fsSL ${SH_URL} | sh`} />
            <CopyCommand label="Windows (PowerShell)" command={`irm ${PS_URL} | iex`} />
          </div>
          <p className="mt-4 text-xs text-zinc-500">
            Why no warning? Files fetched with <code>curl</code>/<code>irm</code> aren&rsquo;t flagged
            &ldquo;downloaded from the internet,&rdquo; so Gatekeeper/SmartScreen never trigger. The
            script is plain text — read it first at{" "}
            <a href={SH_URL} target="_blank" rel="noopener noreferrer" className="text-ember-400 hover:underline">
              install.sh
            </a>{" "}
            /{" "}
            <a href={PS_URL} target="_blank" rel="noopener noreferrer" className="text-ember-400 hover:underline">
              install.ps1
            </a>
            . It detects your OS/arch, installs <code>forge</code> to your PATH, adds the editor panel
            if <code>code</code> is present, and checks for Ollama.
          </p>
        </section>

        <p className="mt-4 text-sm text-zinc-500">
          Signed one-click installers are coming; for now this is the smoothest way in.
        </p>

        {/* Prerequisite */}
        <div className="mt-6 rounded-xl border border-ink-700 bg-ink-900/60 p-4 text-sm text-zinc-400">
          <strong className="text-zinc-200">Requires Ollama</strong> (the local model engine) — install
          from{" "}
          <a href="https://ollama.com/download" target="_blank" rel="noopener noreferrer" className="text-ember-400 hover:underline">
            ollama.com/download
          </a>
          ; the installer checks for it and suggests a model your hardware can run.
        </div>

        {/* PROMINENT first-launch guidance — visible, per-OS, visual (not buried). */}
        <section id="first-launch" className="mt-12 scroll-mt-24">
          <FirstLaunchGuide />
        </section>

        {/* SECONDARY: manual download (also reveals the steps the moment you click). */}
        <details className="mt-10 rounded-2xl border border-ink-700 bg-ink-900/40 p-6">
          <summary className="cursor-pointer text-sm font-semibold text-zinc-300">
            Prefer a manual download? (advanced)
          </summary>
          <p className="mt-3 text-sm text-zinc-400">
            A browser-downloaded bundle <em>is</em> flagged by your OS (the one-liner above avoids
            this). It&rsquo;s safe to run — the build just isn&rsquo;t code-signed yet. The exact
            one-time step for your OS is in{" "}
            <a href="#first-launch" className="text-ember-400 hover:underline">First launch</a> above,
            and appears again the moment you start a download.
          </p>
          <div className="mt-5">
            <DownloadGrid />
          </div>
          <p className="mt-4 text-xs text-zinc-500">
            Each bundle has a SHA-256 link to verify it. Unpack and run the included{" "}
            <code>install.sh</code> / <code>install.ps1</code> — see <code>README-FIRST.md</code> inside.
          </p>
        </details>
      </main>
      <Footer />
    </>
  );
}
