// Public download bundles. The SOURCE repo is private; built (unsigned) binaries
// are published to a PUBLIC releases repo so anonymous visitors can download
// them. URLs use the stable `releases/latest/download/<asset>` form so they
// don't change per version. Override the host with NEXT_PUBLIC_RELEASES_REPO
// (a build-time value on Vercel — set it and REDEPLOY to take effect).
//
// Client-safe (no secrets, no "server-only") so the client download grid can
// import it directly.

const RELEASES_REPO = (
  process.env.NEXT_PUBLIC_RELEASES_REPO || "https://github.com/pranayrishi/ollamax-releases"
).replace(/\/$/, "");

export type Bundle = {
  os: "macos" | "windows" | "linux";
  arch: "arm64" | "x64";
  label: string;
  note: string;
  asset: string;
  /// Whether this asset is actually published in the releases repo. `false`
  /// shows an honest "coming soon" instead of a download link that would 404.
  published: boolean;
};

// Each download is a BUNDLE (forge CLI + VS Code panel .vsix + install script),
// NOT a one-click app — labelled honestly in the UI. `published` reflects which
// assets exist on the latest release (Intel macOS is pending its CI runner).
export const BUNDLES: Bundle[] = [
  { os: "macos", arch: "arm64", label: "macOS — Apple Silicon", note: "M-series · CLI + VS Code panel", asset: "ollama-forge-macos-arm64.tar.gz", published: true },
  { os: "macos", arch: "x64", label: "macOS — Intel", note: "x86_64 · CLI + VS Code panel", asset: "ollama-forge-macos-x64.tar.gz", published: false },
  { os: "windows", arch: "x64", label: "Windows — x64", note: "CLI + VS Code panel", asset: "ollama-forge-windows-x64.zip", published: true },
  { os: "linux", arch: "x64", label: "Linux — x64", note: "CLI + VS Code panel", asset: "ollama-forge-linux-x64.tar.gz", published: true },
];

export function assetUrl(asset: string): string {
  return `${RELEASES_REPO}/releases/latest/download/${asset}`;
}
export function checksumUrl(asset: string): string {
  return `${assetUrl(asset)}.sha256`;
}
export const allReleasesUrl = `${RELEASES_REPO}/releases/latest`;

// The STANDALONE Electron desktop app — the recommended way to get Ollamax: a
// full app with the engine, voice, and login built in. It is distinct from both
// the CLI + editor-extension bundles above and the experimental Code-OSS fork.
//
// Asset names are the exact electron-builder output contract in
// desktop-app/package.json. `published` reflects assets verified on the current
// public latest release; leave a future platform false until its matching asset
// has actually been uploaded, so the site never renders a dead download link.
export type DesktopApp = Bundle;
export const DESKTOP_APPS: DesktopApp[] = [
  { os: "macos", arch: "arm64", label: "macOS — Apple Silicon", note: "M-series · .dmg", asset: "Ollamax-macos-arm64.dmg", published: true },
  { os: "macos", arch: "x64", label: "macOS — Intel", note: "x86_64 · .dmg", asset: "Ollamax-macos-x64.dmg", published: false },
  { os: "windows", arch: "x64", label: "Windows — x64", note: "Installer (.exe)", asset: "Ollamax-windows-x64-setup.exe", published: false },
  { os: "linux", arch: "x64", label: "Linux — x64", note: "AppImage", asset: "Ollamax-linux-x64.AppImage", published: false },
];
