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
    <div className="rounded-2xl border border-ember-500/40 bg-ink-900/60 p-6">
      <div className="flex items-center justify-between">
        <h2 className="text-sm font-semibold uppercase tracking-widest text-ember-400">
          Recommended · the Ollamax app
        </h2>
        {info && <span className="text-xs text-zinc-500">Detected: {info.label}</span>}
      </div>
      <p className="mt-3 text-sm text-zinc-400">
        The full desktop app — engine, on-device voice, and sign-in built in. Nothing else to set up
        but a local{" "}
        <a href="https://ollama.com/download" target="_blank" rel="noopener noreferrer" className="text-ember-400 hover:underline">
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
              className="block rounded-xl bg-ember-500 px-6 py-3.5 text-center font-semibold text-ink-950 transition hover:bg-ember-400"
            >
              Download Ollamax for {primary.label}
            </a>
            <a
              href={checksumUrl(primary.asset)}
              target="_blank"
              rel="noopener noreferrer"
              className="mt-2 block text-center text-[11px] text-zinc-500 hover:text-ember-400"
            >
              SHA-256 checksum
            </a>
          </>
        ) : (
          <div className="rounded-xl border border-ink-700 bg-ink-950/40 px-5 py-4 text-sm text-zinc-300">
            The desktop app for{" "}
            <strong className="text-zinc-100">{info?.label ?? "your platform"}</strong> is{" "}
            <strong className="text-zinc-100">coming soon</strong>. Meanwhile, get the same Ollamax
            experience in your own editor with the{" "}
            <a href="#one-line" className="text-ember-400 hover:underline">one-line installer below</a>.
          </div>
        )}
      </div>

      {/* other platforms */}
      <div className="mt-5 grid gap-2 sm:grid-cols-3">
        {rest.map((b) => (
          <div key={b.asset} className="rounded-xl border border-ink-700 bg-ink-900/50 px-3 py-2.5 text-xs">
            <div className="font-medium text-zinc-200">{b.label}</div>
            {b.published ? (
              <a
                href={assetUrl(b.asset)}
                onClick={() => onDownload(b.os)}
                className="mt-1 inline-block text-ember-400 hover:underline"
              >
                Download
              </a>
            ) : (
              <span className="mt-1 inline-block text-zinc-500">Coming soon</span>
            )}
          </div>
        ))}
      </div>

      <p className="mt-4 text-xs text-zinc-500">
        Unsigned for now, so there&rsquo;s a{" "}
        <Link href="#first-launch" className="text-ember-400 hover:underline">one-time step to open it</Link>{" "}
        — it appears automatically when your download starts.
      </p>

      {/* post-download: first-launch steps, right when they're needed */}
      {startedOS && (
        <div ref={panelRef} className="mt-6 scroll-mt-24 rounded-2xl border border-ember-500/40 bg-ember-500/[0.04] p-1">
          <div className="mb-1 flex items-center gap-2 px-4 pt-3 text-sm font-semibold text-ember-300">
            <span aria-hidden="true">⬇</span> Your download is starting — here&rsquo;s how to open it
          </div>
          <FirstLaunchGuide defaultOS={startedOS} />
        </div>
      )}
    </div>
  );
}
