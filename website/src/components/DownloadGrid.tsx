"use client";

import { useEffect, useRef, useState } from "react";
import { detectOS, type OS, type OSInfo } from "@/lib/os";
import { BUNDLES, assetUrl, checksumUrl, type Bundle } from "@/lib/downloads";
import { FirstLaunchGuide } from "./FirstLaunchGuide";

export function DownloadGrid() {
  const [info, setInfo] = useState<OSInfo | null>(null);
  const [startedOS, setStartedOS] = useState<OS | null>(null);
  const panelRef = useRef<HTMLDivElement>(null);

  // The moment a download starts, reveal the matching first-launch steps and
  // scroll them into view — guidance exactly when it's needed.
  function onDownload(os: OS) {
    setStartedOS(os);
    requestAnimationFrame(() => panelRef.current?.scrollIntoView({ behavior: "smooth", block: "center" }));
  }

  useEffect(() => {
    // Prefer structured high-entropy UA data (the only reliable way to detect
    // Apple Silicon); fall back to the UA string. Never blocks the download.
    const nav = navigator as Navigator & {
      userAgentData?: {
        platform?: string;
        getHighEntropyValues?: (h: string[]) => Promise<{ architecture?: string; platform?: string }>;
      };
    };
    const finish = (platform?: string, arch?: string) =>
      setInfo(detectOS(navigator.userAgent, platform, arch));
    if (nav.userAgentData?.getHighEntropyValues) {
      nav.userAgentData
        .getHighEntropyValues(["architecture", "platform"])
        .then((v) => finish(v.platform || nav.userAgentData?.platform, v.architecture))
        .catch(() => finish(nav.userAgentData?.platform));
    } else {
      finish();
    }
  }, []);

  const detected = info?.os ?? "unknown";
  const detectedArch = info?.arch ?? "unknown";
  const archKnown = detectedArch === "arm64" || detectedArch === "x64";

  // Highlight the matching OS; for a multi-arch OS (macOS) only highlight a card
  // when we actually know the arch (high-entropy hint). On Safari/Firefox the
  // arch is unknown, so we highlight nothing and tell the user to pick their chip
  // rather than falsely claiming one is highlighted.
  const isMatch = (b: Bundle) =>
    b.os === detected &&
    (BUNDLES.filter((x) => x.os === b.os).length === 1 || (archKnown && detectedArch === b.arch));
  const anyMatch = detected !== "unknown" && BUNDLES.some(isMatch);
  const osOnly = detected !== "unknown" && !anyMatch;

  return (
    <div>
      {anyMatch && (
        <p className="mb-5 text-sm text-muted-foreground">
          Detected: <span className="text-foreground">{info!.label}</span> — the matching download is
          highlighted. All options are listed; pick whichever you need.
        </p>
      )}
      {osOnly && (
        <p className="mb-5 text-sm text-muted-foreground">
          Detected: <span className="text-foreground">{info!.label}</span> — pick your chip below
          (Apple Silicon for M-series, Intel for older Macs). All options are listed.
        </p>
      )}
      <div className="grid gap-3 sm:grid-cols-2">
        {BUNDLES.map((b) => {
          const match = isMatch(b);
          const url = assetUrl(b.asset);
          return (
            <div
              key={b.asset}
              className={`surface-subtle p-5 ${match ? "bg-muted" : ""}`}
            >
              <div className="flex items-center justify-between">
                <span className="font-medium text-foreground">{b.label}</span>
                {match && <span className="text-xs text-foreground">recommended</span>}
              </div>
              <p className="mt-2 text-xs text-muted-foreground">{b.note}</p>
              {b.published ? (
                <>
                  <a
                    href={url}
                    onClick={() => onDownload(b.os)}
                    className="button-primary mt-5 block w-full px-4 py-2 text-center"
                  >
                    Download
                  </a>
                  <a
                    href={checksumUrl(b.asset)}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="mt-3 block text-center text-[11px] text-muted-foreground transition-colors hover:text-foreground"
                  >
                    SHA-256 checksum
                  </a>
                </>
              ) : (
                <span
                  className="mt-5 block cursor-not-allowed rounded-full bg-muted px-4 py-2 text-center text-sm text-muted-foreground"
                  title="This build isn't published yet"
                >
                  Coming soon
                </span>
              )}
            </div>
          );
        })}
      </div>

      {/* Post-download: the steps appear the instant a download starts. */}
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
