import type { Metadata } from "next";
import { Nav } from "@/components/Nav";
import { Footer } from "@/components/Footer";
import { DownloadGrid, type Installer } from "@/components/DownloadGrid";

export const metadata: Metadata = { title: "Download" };

// Installer URLs + checksums come from env (the release pipeline publishes them).
// Unset → "coming soon". macOS is a signed+notarized universal build; Windows is
// Authenticode-signed; Linux ships AppImage + .deb.
const env = process.env;
const installers: Installer[] = [
  {
    os: "macos",
    label: "macOS — Universal (.dmg)",
    note: "Apple Silicon + Intel · Developer ID signed + notarized",
    url: env.NEXT_PUBLIC_DOWNLOAD_MACOS || null,
    sha256: env.NEXT_PUBLIC_DOWNLOAD_MACOS_SHA256 || null,
  },
  {
    os: "windows",
    label: "Windows — x64 (installer)",
    note: "NSIS · Authenticode-signed",
    url: env.NEXT_PUBLIC_DOWNLOAD_WINDOWS || null,
    sha256: env.NEXT_PUBLIC_DOWNLOAD_WINDOWS_SHA256 || null,
  },
  {
    os: "linux",
    label: "Linux — .AppImage",
    note: "Portable · x64",
    url: env.NEXT_PUBLIC_DOWNLOAD_LINUX || null,
    sha256: env.NEXT_PUBLIC_DOWNLOAD_LINUX_SHA256 || null,
  },
  {
    os: "linux",
    label: "Linux — .deb",
    note: "Debian / Ubuntu · x64",
    url: env.NEXT_PUBLIC_DOWNLOAD_LINUX_DEB || null,
    sha256: env.NEXT_PUBLIC_DOWNLOAD_LINUX_DEB_SHA256 || null,
  },
];

export default function DownloadPage() {
  return (
    <>
      <Nav />
      <main id="main" className="mx-auto max-w-3xl px-4 py-16">
        <h1 className="text-3xl font-bold tracking-tight text-zinc-50">Download Ollama-Forge</h1>
        <p className="mt-3 text-zinc-400">
          Free and open source. Requires a local Ollama daemon (the app helps you set it up on first
          run). Your code stays on your machine; we collect only anonymous usage metadata you can turn
          off.
        </p>
        <div className="mt-8">
          <DownloadGrid installers={installers} />
        </div>
        <p className="mt-8 text-sm text-zinc-500">
          Verify your download against the SHA-256 checksum shown on each build. Signed/notarized so
          your OS trusts it without warnings.
        </p>
      </main>
      <Footer />
    </>
  );
}
