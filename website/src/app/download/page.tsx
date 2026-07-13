import type { Metadata } from "next";
import { Nav } from "@/components/Nav";
import { Footer } from "@/components/Footer";
import { DownloadGrid } from "@/components/DownloadGrid";
import { DesktopAppDownload } from "@/components/DesktopAppDownload";
import { FirstLaunchGuide } from "@/components/FirstLaunchGuide";

export const metadata: Metadata = { title: "Download" };

export default function DownloadPage() {
  return (
    <>
      <Nav />
      <main id="main" className="page-frame max-w-3xl">
        <h1 className="page-title">Download Ollamax</h1>
        <p className="page-lede">
          Free and open source. The source tree adds optional local voice, a visual-only cursor cue, screen-region
          context, explicit loopback model endpoints, and an updated model catalog. The public v0.2.0 downloads do
          not include those changes, so all package controls remain disabled until replacement artifacts are verified.
        </p>

        {/* PRIMARY: the standalone desktop app */}
        <div className="mt-8">
          <DesktopAppDownload />
        </div>

        {/* Keep stale CLI/editor commands off the page until a complete matching release exists. */}
        <section id="one-line" className="surface mt-10 scroll-mt-24">
          <h2 className="eyebrow">
            Editor bundle · awaiting the matching verified release
          </h2>
          <p className="mt-4 text-sm leading-relaxed text-muted-foreground">
            The CLI and VS Code bundle will be enabled only when the matching release has passed its complete
            desktop/CLI asset check. The site intentionally does not show a <code>latest</code> install command
            that could fetch the older public v0.2.0 build instead of these source-tree changes.
          </p>
        </section>

        <p className="mt-5 text-sm text-muted-foreground">
          Replacement installers and editor bundles are enabled only after their matching public release is visible
          and its checksums have been verified.
        </p>

        {/* Prerequisite */}
        <div className="surface-subtle mt-8 p-5 text-sm leading-relaxed text-muted-foreground">
          <strong className="text-foreground">Ollama is the default local model engine</strong> — install
          from{" "}
          <a href="https://ollama.com/download" target="_blank" rel="noopener noreferrer" className="text-link">
            ollama.com/download
          </a>
          ; the installer checks for it and suggests a model your hardware can run. An advanced, separately operated
          server may instead be configured at a literal loopback endpoint; no cloud provider is chosen automatically.
          Local voice recognition additionally needs a local Whisper runtime unless a particular package has staged
          one; the app shows setup rather than using a hosted speech provider when it is absent.
        </div>

        {/* PROMINENT first-launch guidance — visible, per-OS, visual (not buried). */}
        <section id="first-launch" className="mt-12 scroll-mt-24">
          <FirstLaunchGuide />
        </section>

        {/* SECONDARY: manual download (also reveals the steps the moment you click). */}
        <details className="surface mt-12">
          <summary className="cursor-pointer text-sm font-medium text-foreground">
            Prefer a manual download? (advanced)
          </summary>
          <p className="mt-4 text-sm leading-relaxed text-muted-foreground">
            A browser-downloaded bundle may be flagged by your OS because the current packages are not
            code-signed. Once a verified package is enabled, the exact one-time step for your OS is in{" "}
            <a href="#first-launch" className="text-link">First launch</a> above,
            and appears again the moment you start a download.
          </p>
          <div className="mt-5">
            <DownloadGrid />
          </div>
          <p className="mt-5 text-xs leading-relaxed text-muted-foreground">
            Every enabled bundle will have a SHA-256 link. Do not treat a disabled card or an older
            public asset as the source-tree release described above.
          </p>
        </details>
      </main>
      <Footer />
    </>
  );
}
