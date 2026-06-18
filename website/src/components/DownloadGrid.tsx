"use client";

import { useEffect, useState } from "react";
import { detectOS, type OSInfo } from "@/lib/os";
import { BUNDLES, assetUrl, checksumUrl, type Bundle } from "@/lib/downloads";

export function DownloadGrid() {
  const [info, setInfo] = useState<OSInfo | null>(null);

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
        <p className="mb-4 text-sm text-zinc-400">
          Detected: <span className="text-ember-400">{info!.label}</span> — the matching download is
          highlighted. All options are listed; pick whichever you need.
        </p>
      )}
      {osOnly && (
        <p className="mb-4 text-sm text-zinc-400">
          Detected: <span className="text-ember-400">{info!.label}</span> — pick your chip below
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
              className={`rounded-2xl border p-5 ${
                match ? "border-ember-500 bg-ink-800" : "border-ink-700 bg-ink-900/60"
              }`}
            >
              <div className="flex items-center justify-between">
                <span className="font-semibold text-zinc-100">{b.label}</span>
                {match && <span className="text-xs text-ember-400">recommended</span>}
              </div>
              <p className="mt-1 text-xs text-zinc-500">{b.note}</p>
              {b.published ? (
                <>
                  <a
                    href={url}
                    className="mt-4 block rounded-lg bg-ember-500 px-4 py-2 text-center text-sm font-semibold text-ink-950 hover:bg-ember-400"
                  >
                    Download
                  </a>
                  <a
                    href={checksumUrl(b.asset)}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="mt-2 block text-center text-[11px] text-zinc-500 hover:text-ember-400"
                  >
                    SHA-256 checksum
                  </a>
                </>
              ) : (
                <span
                  className="mt-4 block cursor-not-allowed rounded-lg bg-ink-800 px-4 py-2 text-center text-sm text-zinc-500"
                  title="This build isn't published yet"
                >
                  Coming soon
                </span>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
