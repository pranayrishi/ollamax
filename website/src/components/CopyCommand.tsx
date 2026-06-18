"use client";

import { useState } from "react";

/// A copy-to-clipboard command block — the primary install path. Shows the
/// one-liner and a Copy button; falls back to select-all on clipboard failure.
export function CopyCommand({ command, label }: { command: string; label?: string }) {
  const [copied, setCopied] = useState(false);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(command);
      setCopied(true);
      setTimeout(() => setCopied(false), 1800);
    } catch {
      // Clipboard blocked (e.g. insecure context) — select the text instead.
      const el = document.getElementById("cmd-" + slug(command));
      if (el) {
        const r = document.createRange();
        r.selectNodeContents(el);
        const sel = window.getSelection();
        sel?.removeAllRanges();
        sel?.addRange(r);
      }
    }
  };

  return (
    <div>
      {label && <div className="mb-1.5 text-xs font-medium text-zinc-400">{label}</div>}
      <div className="flex items-stretch gap-2">
        <code
          id={"cmd-" + slug(command)}
          className="flex-1 overflow-x-auto whitespace-nowrap rounded-lg border border-ink-700 bg-ink-950 px-3 py-2.5 font-mono text-xs text-zinc-200"
        >
          {command}
        </code>
        <button
          onClick={copy}
          className="shrink-0 rounded-lg bg-ember-500 px-4 text-sm font-semibold text-ink-950 hover:bg-ember-400"
          aria-label="Copy command"
        >
          {copied ? "Copied ✓" : "Copy"}
        </button>
      </div>
    </div>
  );
}

function slug(s: string): string {
  return s.replace(/[^a-z0-9]+/gi, "").slice(0, 24);
}
