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
      {label && <div className="mb-2 text-xs font-medium text-muted-foreground">{label}</div>}
      <div className="flex items-stretch gap-2">
        <code
          id={"cmd-" + slug(command)}
          className="flex-1 overflow-x-auto whitespace-nowrap rounded-2xl border border-border bg-background px-4 py-3 font-mono text-xs text-foreground/85"
        >
          {command}
        </code>
        <button
          onClick={copy}
          className="button-primary shrink-0 px-4"
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
