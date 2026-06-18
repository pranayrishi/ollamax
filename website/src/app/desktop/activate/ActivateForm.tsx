"use client";

import { useState } from "react";

// Two-step, anti-phishing device approval:
//  1. The user TYPES the code shown in their own app (we never prefill it).
//  2. We show WHAT is requesting access (when + which app/browser) and require
//     an explicit, informed confirmation before binding their account.
// This defeats the one-click "approve a code someone linked you" attack.
export function ActivateForm() {
  const [step, setStep] = useState<"enter" | "confirm" | "ok">("enter");
  const [code, setCode] = useState("");
  const [info, setInfo] = useState<{ createdAt?: string; userAgent?: string | null }>({});
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");

  async function lookup(e: React.FormEvent) {
    e.preventDefault();
    setErr("");
    setBusy(true);
    try {
      const res = await fetch("/api/desktop/device/info", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ user_code: code.trim().toUpperCase() }),
      });
      const data = await res.json();
      if (!res.ok) {
        setErr("Could not look up that code.");
      } else if (!data.found) {
        setErr("That code is invalid or expired. Check the code shown in your app.");
      } else {
        setInfo({ createdAt: data.createdAt, userAgent: data.userAgent });
        setStep("confirm");
      }
    } catch {
      setErr("Network error.");
    } finally {
      setBusy(false);
    }
  }

  async function approve() {
    setErr("");
    setBusy(true);
    try {
      const res = await fetch("/api/desktop/device/approve", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ user_code: code.trim().toUpperCase() }),
      });
      const data = await res.json();
      if (res.ok && data.ok) setStep("ok");
      else setErr("That code is invalid or expired.");
    } catch {
      setErr("Network error.");
    } finally {
      setBusy(false);
    }
  }

  if (step === "ok") {
    return (
      <div className="mt-8 rounded-2xl border border-ember-500/40 bg-ember-500/10 p-6 text-center">
        <p className="text-lg font-semibold text-zinc-100">✓ App linked</p>
        <p className="mt-1 text-sm text-zinc-400">
          Return to the desktop app — it will finish signing in automatically.
        </p>
      </div>
    );
  }

  if (step === "confirm") {
    const when = info.createdAt ? new Date(info.createdAt).toLocaleTimeString() : "just now";
    return (
      <div className="mt-8 space-y-4">
        <div className="rounded-xl border border-ink-700 bg-ink-900/70 p-4 text-sm">
          <p className="text-zinc-300">A device is requesting access to your account:</p>
          <ul className="mt-2 space-y-1 text-zinc-400">
            <li>· Started: {when}</li>
            <li>· Requesting app: {info.userAgent || "unknown"}</li>
          </ul>
        </div>
        <div className="rounded-xl border border-amber-500/40 bg-amber-500/10 p-4 text-sm text-amber-200">
          ⚠ Only approve if <strong>you</strong> just started signing in on your own device. Never
          approve a code that someone sent you — doing so would give them access to your account.
        </div>
        {err && <p className="text-sm text-red-400">{err}</p>}
        <div className="flex gap-3">
          <button
            onClick={approve}
            disabled={busy}
            className="flex-1 rounded-xl bg-ember-500 px-5 py-3 font-semibold text-ink-950 hover:bg-ember-400 disabled:opacity-60"
          >
            {busy ? "Linking…" : "Yes, this is my device — link it"}
          </button>
          <button
            onClick={() => {
              setStep("enter");
              setErr("");
            }}
            className="rounded-xl border border-ink-600 bg-ink-800 px-4 py-3 text-sm text-zinc-300"
          >
            Cancel
          </button>
        </div>
      </div>
    );
  }

  return (
    <form onSubmit={lookup} className="mt-8 space-y-4">
      <p className="rounded-xl border border-amber-500/30 bg-amber-500/[0.06] p-3 text-xs text-amber-200/90">
        Type the code shown in <strong>your</strong> app. Don&rsquo;t enter a code someone gave you.
      </p>
      <label htmlFor="user_code" className="block text-sm font-medium text-zinc-300">
        Device code
      </label>
      <input
        id="user_code"
        name="user_code"
        value={code}
        onChange={(e) => setCode(e.target.value)}
        placeholder="WXYZ-1234"
        autoComplete="off"
        autoCapitalize="characters"
        className="w-full rounded-xl border border-ink-600 bg-ink-800 px-4 py-3 text-center font-mono text-lg tracking-widest text-zinc-100 placeholder:text-zinc-600 focus:border-ember-500"
        required
      />
      {err && <p className="text-sm text-red-400">{err}</p>}
      <button
        type="submit"
        disabled={busy}
        className="w-full rounded-xl bg-ember-500 px-5 py-3 font-semibold text-ink-950 hover:bg-ember-400 disabled:opacity-60"
      >
        {busy ? "Checking…" : "Continue"}
      </button>
    </form>
  );
}
