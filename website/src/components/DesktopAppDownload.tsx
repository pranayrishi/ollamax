"use client";

import { useEffect, useRef, useState } from "react";
import Link from "next/link";
import { detectOS, type OS, type OSInfo } from "@/lib/os";
import { DESKTOP_APPS, assetUrl, checksumUrl, type DesktopApp } from "@/lib/downloads";
import { FirstLaunchGuide } from "./FirstLaunchGuide";

// Prominent primary download for the STANDALONE Ollamax app. Detects the
// visitor's OS/arch, leads with the matching build, and reveals the one-time
// first-launch steps the instant a download starts.
export function DesktopAppDownload() {
  const [info, setInfo] = useState<OSInfo | null>(null);
  const [startedOS, setStartedOS] = useState<OS | null>(null);
  const panelRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const nav = navigator as Navigator & {
      userAgentData?: {
        platform?: string;
        getHighEntropyValues?: (h: string[]) => Promise<{ architecture?: string; platform?: string }>;
      };
    };
    const finish = (platform?: string, arch?: string) => setInfo(detectOS(navigator.userAgent, platform, arch));
    if (nav.userAgentData?.getHighEntropyValues) {
      nav.userAgentData
        .getHighEntropyValues(["architecture", "platform"])
        .then((v) => finish(v.platform || nav.userAgentData?.platform, v.architecture))
        .catch(() => finish(nav.userAgentData?.platform));
    } else {
      finish();
    }
  }, []);

  function onDownload(os: OS) {
    setStartedOS(os);
    requestAnimationFrame(() => panelRef.current?.scrollIntoView({ behavior: "smooth", block: "center" }));
  }

  const detected = info?.os ?? "unknown";
  const detectedArch = info?.arch ?? "unknown";
  const archKnown = detectedArch === "arm64" || detectedArch === "x64";
  const isMatch = (b: DesktopApp) =>
    b.os === detected &&
    (DESKTOP_APPS.filter((x) => x.os === b.os).length === 1 || (archKnown && detectedArch === b.arch));

  // Lead with the best build for the detected platform (prefer a published match).
  const primary =
    DESKTOP_APPS.find((b) => isMatch(b) && b.published) ??
    DESKTOP_APPS.find((b) => isMatch(b)) ??
    null;
  const rest = DESKTOP_APPS.filter((b) => b !== primary);

  return (
    <div className="surface">
      <div className="flex items-center justify-between">
        <h2 className="eyebrow">
          Recommended · the Ollamax app
        </h2>
        {info && <span className="text-xs text-muted-foreground">Detected: {info.label}</span>}
      </div>
      <p className="mt-4 text-sm leading-relaxed text-muted-foreground">
        The full desktop app — engine, the voice + screen companion, and sign-in built in. Nothing
        else to set up but a local{" "}
        <a href="https://ollama.com/download" target="_blank" rel="noopener noreferrer" className="text-link">
          Ollama
        </a>
        .
      </p>

      {/* primary action */}
      <div className="mt-5">
        {primary && primary.published ? (
          <>
            <a
              href={assetUrl(primary.asset)}
              onClick={() => onDownload(primary.os)}
              className="button-primary block w-full px-6 py-3.5 text-center"
            >
              Download Ollamax for {primary.label}
            </a>
            <a
              href={checksumUrl(primary.asset)}
              target="_blank"
              rel="noopener noreferrer"
              className="mt-3 block text-center text-[11px] text-muted-foreground transition-colors hover:text-foreground"
            >
              SHA-256 checksum
            </a>
          </>
        ) : (
          <div className="surface-subtle px-5 py-4 text-sm text-foreground/85">
            The desktop app for{" "}
            <strong className="text-foreground">{info?.label ?? "your platform"}</strong> is{" "}
            <strong className="text-foreground">coming soon</strong>. Meanwhile, get the same Ollamax
            experience in your own editor with the{" "}
            <a href="#one-line" className="text-link">one-line installer below</a>.
          </div>
        )}
      </div>

      {/* other platforms */}
      <div className="mt-5 grid gap-2 sm:grid-cols-3">
        {rest.map((b) => (
          <div key={b.asset} className="surface-subtle px-4 py-3 text-xs">
            <div className="font-medium text-foreground">{b.label}</div>
            {b.published ? (
              <a
                href={assetUrl(b.asset)}
                onClick={() => onDownload(b.os)}
                className="mt-2 inline-block text-foreground underline decoration-muted-foreground/60 underline-offset-4"
              >
                Download
              </a>
            ) : (
              <span className="mt-2 inline-block text-muted-foreground">Coming soon</span>
            )}
          </div>
        ))}
      </div>

      <p className="mt-5 text-xs leading-relaxed text-muted-foreground">
        Unsigned for now, so there&rsquo;s a{" "}
        <Link href="#first-launch" className="text-link">one-time step to open it</Link>{" "}
        — it appears automatically when your download starts.
      </p>

      {/* post-download: first-launch steps, right when they're needed */}
      {startedOS && (
        <div ref={panelRef} className="surface mt-6 scroll-mt-24 p-1">
          <div className="mb-1 flex items-center gap-2 px-4 pt-3 text-sm font-medium text-foreground">
            <span aria-hidden="true">⬇</span> Your download is starting — here&rsquo;s how to open it
          </div>
          <FirstLaunchGuide defaultOS={startedOS} />
        </div>
      )}
    </div>
  );
}
