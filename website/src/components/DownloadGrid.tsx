"use client";

import { useEffect, useState } from "react";
import { detectOS, type OSInfo } from "@/lib/os";

// Fallback target when an installer isn't published yet (NEXT_PUBLIC_* inlined
// at build time). Points at GitHub Releases so the card is never a dead no-op.
const repo = process.env.NEXT_PUBLIC_GITHUB_REPO;
const releasesUrl = repo ? `${repo.replace(/\/$/, "")}/releases` : null;

export type Installer = {
  os: "macos" | "windows" | "linux";
  label: string;
  note: string;
  url: string | null;
  sha256: string | null;
};

export function DownloadGrid({ installers }: { installers: Installer[] }) {
  const [info, setInfo] = useState<OSInfo | null>(null);

  useEffect(() => {
    // Prefer structured high-entropy UA data (the only reliable way to detect
    // Apple Silicon); fall back to the UA string. Never blocks the download.
    const nav = navigator as Navigator & {
      userAgentData?: { platform?: string; getHighEntropyValues?: (h: string[]) => Promise<{ architecture?: string; platform?: string }> };
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

  return (
    <div>
      {info && info.os !== "unknown" && (
        <p className="mb-4 text-sm text-zinc-400">
          Detected: <span className="text-ember-400">{info.label}</span> — we&rsquo;ve highlighted the
          matching download. All options are listed below.
        </p>
      )}
      <div className="grid gap-3 sm:grid-cols-2">
        {installers.map((it, i) => {
          const match = it.os === detected;
          const available = !!it.url;
          return (
            <div
              key={i}
              className={`rounded-2xl border p-5 ${
                match ? "border-ember-500 bg-ink-800" : "border-ink-700 bg-ink-900/60"
              }`}
            >
              <div className="flex items-center justify-between">
                <span className="font-semibold text-zinc-100">{it.label}</span>
                {match && <span className="text-xs text-ember-400">recommended</span>}
              </div>
              <p className="mt-1 text-xs text-zinc-500">{it.note}</p>
              {available ? (
                <a
                  href={it.url!}
                  className="mt-4 block rounded-lg bg-ember-500 px-4 py-2 text-center text-sm font-semibold text-ink-950 hover:bg-ember-400"
                >
                  Download
                </a>
              ) : releasesUrl ? (
                <a
                  href={releasesUrl}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="mt-4 block rounded-lg border border-ink-600 bg-ink-800 px-4 py-2 text-center text-sm font-semibold text-zinc-200 hover:border-ember-500"
                >
                  Coming soon — view releases ↗
                </a>
              ) : (
                <span className="mt-4 block cursor-not-allowed rounded-lg bg-ink-800 px-4 py-2 text-center text-sm text-zinc-500">
                  Coming soon
                </span>
              )}
              {it.sha256 && (
                <p className="mt-2 break-all font-mono text-[10px] text-zinc-600" title="SHA-256 checksum">
                  sha256: {it.sha256}
                </p>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
