"use client";

import { useEffect, useState } from "react";
import { detectOS, type OS } from "@/lib/os";

// First-launch guidance for the UNSIGNED app. Honest + reassuring: the OS shows a
// ONE-TIME prompt *because* the app isn't code-signed yet (signed installers are
// planned). We auto-select the visitor's OS and show the exact clicks, with a
// simple illustrative mock of the dialog (not a real screenshot, not a false
// "verified" claim).

type Step = { n: number; title: string; body: string };
type Guide = { os: OS; label: string; lead: string; steps: Step[]; fallback?: string };

const GUIDES: Guide[] = [
  {
    os: "macos",
    label: "macOS",
    lead: "Right-click to open it the first time — that's the whole trick.",
    steps: [
      {
        n: 1,
        title: "Right-click Ollamax → Open",
        body: "In Applications, Control-click (or right-click) the Ollamax app and choose Open. Don't double-click it the first time.",
      },
      {
        n: 2,
        title: "Click “Open” in the dialog",
        body: "macOS asks to confirm because the app isn't code-signed yet. Click Open. This happens once.",
      },
      {
        n: 3,
        title: "Done — it opens normally after",
        body: "From now on Ollamax launches with a normal double-click, like any other app.",
      },
    ],
    fallback:
      "On the latest macOS? If there's no Open button, double-click once, then go to System Settings → Privacy & Security and click “Open Anyway”.",
  },
  {
    os: "windows",
    label: "Windows",
    lead: "Two clicks past the SmartScreen notice — one time only.",
    steps: [
      {
        n: 1,
        title: "Click “More info”",
        body: "On the blue “Windows protected your PC” screen, click the small More info link.",
      },
      {
        n: 2,
        title: "Click “Run anyway”",
        body: "SmartScreen shows this because the app is new and not yet signed. Click Run anyway.",
      },
      {
        n: 3,
        title: "Done — launches normally after",
        body: "Windows remembers your choice and won't prompt again.",
      },
    ],
  },
  {
    os: "linux",
    label: "Linux",
    lead: "No signing prompt — just make it executable.",
    steps: [
      {
        n: 1,
        title: "Allow executing",
        body: "Run chmod +x on the binary/AppImage, or use your file manager → Properties → “Allow executing file as program”.",
      },
      { n: 2, title: "Run it", body: "Launch Ollamax. Linux doesn't gate unsigned apps." },
    ],
  },
];

/** Illustrative (not a real screenshot) mock of the OS dialog, with the button to click highlighted. */
function DialogMock({ os }: { os: OS }) {
  if (os === "windows") {
    return (
      <svg viewBox="0 0 320 200" className="h-auto w-full" role="img" aria-label="Illustration: Windows SmartScreen — click More info, then Run anyway">
        <rect width="320" height="200" rx="10" fill="#1b2330" stroke="#2b3647" />
        <rect x="0" y="0" width="320" height="46" rx="10" fill="#0b3a82" />
        <rect x="0" y="30" width="320" height="16" fill="#0b3a82" />
        <text x="20" y="29" fill="#fff" fontSize="13" fontWeight="700">Windows protected your PC</text>
        <text x="20" y="74" fill="#9fb0c3" fontSize="10.5">Microsoft Defender SmartScreen prevented an</text>
        <text x="20" y="90" fill="#9fb0c3" fontSize="10.5">unrecognized app from starting.</text>
        <text x="20" y="118" fill="#f59e3c" fontSize="11" fontWeight="600" textDecoration="underline">More info</text>
        <rect x="186" y="150" width="116" height="32" rx="6" fill="#f59e3c" />
        <text x="244" y="171" fill="#1a1205" fontSize="12" fontWeight="700" textAnchor="middle">Run anyway</text>
        <rect x="186" y="150" width="116" height="32" rx="6" fill="none" stroke="#ffd9a8" strokeWidth="2">
          <animate attributeName="opacity" values="1;0.3;1" dur="1.8s" repeatCount="indefinite" />
        </rect>
      </svg>
    );
  }
  if (os === "linux") {
    return (
      <svg viewBox="0 0 320 200" className="h-auto w-full" role="img" aria-label="Illustration: mark the file executable, then run it">
        <rect width="320" height="200" rx="10" fill="#0c1118" stroke="#2b3647" />
        <rect x="0" y="0" width="320" height="34" rx="10" fill="#161d29" />
        <rect x="0" y="20" width="320" height="14" fill="#161d29" />
        <circle cx="18" cy="17" r="4" fill="#ff5f57" /><circle cx="32" cy="17" r="4" fill="#febc2e" /><circle cx="46" cy="17" r="4" fill="#28c840" />
        <text x="20" y="70" fill="#7dd3a8" fontSize="12" fontFamily="monospace">$ chmod +x Ollamax</text>
        <text x="20" y="96" fill="#7dd3a8" fontSize="12" fontFamily="monospace">$ ./Ollamax</text>
        <text x="20" y="130" fill="#6b7a8d" fontSize="11">No signature prompt on Linux.</text>
      </svg>
    );
  }
  // macOS
  return (
    <svg viewBox="0 0 320 200" className="h-auto w-full" role="img" aria-label="Illustration: macOS asks to confirm — click Open">
      <rect width="320" height="200" rx="10" fill="#1f2430" stroke="#2b3647" />
      <circle cx="160" cy="52" r="22" fill="#2a3140" stroke="#3a4456" />
      <text x="160" y="59" fontSize="20" textAnchor="middle">⚒</text>
      <text x="160" y="98" fill="#e6ebf2" fontSize="11.5" fontWeight="600" textAnchor="middle">macOS cannot verify the developer</text>
      <text x="160" y="114" fill="#9fb0c3" fontSize="10.5" textAnchor="middle">of “Ollamax”. Open it anyway?</text>
      <rect x="44" y="150" width="100" height="32" rx="7" fill="#2a3140" stroke="#3a4456" />
      <text x="94" y="171" fill="#c5cedb" fontSize="12" textAnchor="middle">Cancel</text>
      <rect x="176" y="150" width="100" height="32" rx="7" fill="#f59e3c" />
      <text x="226" y="171" fill="#1a1205" fontSize="12" fontWeight="700" textAnchor="middle">Open</text>
      <rect x="176" y="150" width="100" height="32" rx="7" fill="none" stroke="#ffd9a8" strokeWidth="2">
        <animate attributeName="opacity" values="1;0.3;1" dur="1.8s" repeatCount="indefinite" />
      </rect>
    </svg>
  );
}

export function FirstLaunchGuide({ defaultOS }: { defaultOS?: OS }) {
  const [active, setActive] = useState<OS>(defaultOS ?? "macos");
  const [detected, setDetected] = useState(false);

  useEffect(() => {
    // When an explicit OS is provided (e.g. the bundle a visitor just downloaded),
    // honor it and skip auto-detection.
    if (detected || defaultOS) return;
    const info = detectOS(navigator.userAgent, (navigator as Navigator & { userAgentData?: { platform?: string } }).userAgentData?.platform);
    if (info.os !== "unknown") setActive(info.os);
    setDetected(true);
  }, [detected, defaultOS]);

  const guide = GUIDES.find((g) => g.os === active) ?? GUIDES[0];

  return (
    <div className="rounded-2xl border border-ink-700 bg-ink-900/60 p-6 sm:p-8">
      <div className="flex flex-col gap-4 sm:flex-row sm:items-start sm:justify-between">
        <div>
          <h3 className="text-lg font-semibold text-zinc-100">Opening it the first time</h3>
          <p className="mt-2 max-w-xl text-sm leading-relaxed text-zinc-400">
            Because Ollamax is a new, independent app that <strong className="text-zinc-200">isn&rsquo;t code-signed yet</strong>,
            your system shows a <strong className="text-zinc-200">one-time</strong> security prompt the first time you open it.
            That&rsquo;s expected for new software — here&rsquo;s the single step to get past it. Signed installers are on the way.
          </p>
        </div>
        {/* OS switcher */}
        <div className="flex shrink-0 gap-1 rounded-xl border border-ink-700 bg-ink-950/60 p-1 text-xs">
          {GUIDES.map((g) => (
            <button
              key={g.os}
              type="button"
              onClick={() => setActive(g.os)}
              className={`rounded-lg px-3 py-1.5 font-medium transition ${
                active === g.os ? "bg-ember-500 text-ink-950" : "text-zinc-400 hover:text-zinc-100"
              }`}
            >
              {g.label}
            </button>
          ))}
        </div>
      </div>

      <p className="mt-5 text-sm font-medium text-ember-300">{guide.lead}</p>

      <div className="mt-4 grid gap-6 md:grid-cols-[1fr_300px] md:items-center">
        <ol className="space-y-4">
          {guide.steps.map((s) => (
            <li key={s.n} className="flex gap-3">
              <span className="mt-0.5 grid h-6 w-6 shrink-0 place-items-center rounded-full border border-ember-500/40 bg-ember-500/10 text-xs font-semibold text-ember-300">
                {s.n}
              </span>
              <div>
                <p className="text-sm font-semibold text-zinc-100">{s.title}</p>
                <p className="text-sm leading-relaxed text-zinc-400">{s.body}</p>
              </div>
            </li>
          ))}
        </ol>
        <div className="rounded-xl border border-ink-700 bg-ink-950/40 p-3">
          <DialogMock os={active} />
          <p className="mt-2 text-center text-[11px] text-zinc-600">Illustration — the actual dialog may vary by OS version.</p>
        </div>
      </div>

      {guide.fallback && (
        <p className="mt-5 rounded-lg border border-ink-700 bg-ink-950/40 px-4 py-3 text-xs text-zinc-400">
          <strong className="text-zinc-300">Recent macOS?</strong> {guide.fallback}
        </p>
      )}

      <p className="mt-4 text-xs text-zinc-500">
        Prefer to verify your download? Every build ships a{" "}
        <strong className="text-zinc-400">SHA-256 checksum</strong> next to it — compare it to be sure the file is intact.
        We don&rsquo;t claim the app is &ldquo;verified&rdquo;; the prompt simply means it isn&rsquo;t signed yet.
      </p>
    </div>
  );
}
