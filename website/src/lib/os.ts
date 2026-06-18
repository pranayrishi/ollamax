// OS + architecture detection for the download page. Pure + unit-tested.
// Prefer the structured `navigator.userAgentData` high-entropy values (passed in
// as hints); fall back to the UA string. NOTE: on Apple Silicon the UA string
// always claims "Intel", so arm64 on macOS is only reliable from the
// high-entropy `architecture` hint — otherwise we report arch "unknown" and
// suggest the universal build. Detection never blocks the download.

export type OS = "macos" | "windows" | "linux" | "unknown";
export type Arch = "arm64" | "x64" | "unknown";
export type OSInfo = { os: OS; arch: Arch; label: string };

export function detectOS(ua: string, platformHint?: string, archHint?: string): OSInfo {
  const u = (ua || "").toLowerCase();

  let os: OS = "unknown";
  const p = (platformHint || "").toLowerCase();
  if (p.includes("mac")) os = "macos";
  else if (p.includes("win")) os = "windows";
  else if (p.includes("linux") || p.includes("chrome os")) os = "linux";
  if (os === "unknown") {
    if (u.includes("mac os") || u.includes("macintosh")) os = "macos";
    else if (u.includes("windows")) os = "windows";
    else if (u.includes("linux") || u.includes("x11")) os = "linux";
  }

  let arch: Arch = "unknown";
  const a = (archHint || "").toLowerCase();
  if (a.includes("arm")) arch = "arm64";
  else if (a.includes("x86") || a.includes("amd64") || a === "x64") arch = "x64";
  if (arch === "unknown") {
    if (u.includes("arm64") || u.includes("aarch64")) arch = "arm64";
    else if (u.includes("x86_64") || u.includes("win64") || u.includes("wow64") || u.includes("x64"))
      arch = "x64";
  }

  return { os, arch, label: labelFor(os, arch) };
}

export function labelFor(os: OS, arch: Arch): string {
  const osName = os === "macos" ? "macOS" : os === "windows" ? "Windows" : os === "linux" ? "Linux" : "your platform";
  if (os === "macos") return arch === "arm64" ? "macOS — Apple Silicon" : arch === "x64" ? "macOS — Intel" : "macOS — Universal";
  if (os === "windows") return arch === "arm64" ? "Windows — ARM64" : "Windows — x64";
  if (os === "linux") return arch === "arm64" ? "Linux — ARM64" : "Linux — x64";
  return osName;
}
