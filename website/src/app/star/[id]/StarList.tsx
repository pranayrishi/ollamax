"use client";

import { useState } from "react";

type Repo = { full_name: string; html_url: string; license_spdx: string | null };

export function StarList({ id, repos }: { id: string; repos: Repo[] }) {
  // All pre-selected; the user can deselect any. Conscious, explicit choice.
  const [selected, setSelected] = useState<Set<string>>(new Set(repos.map((r) => r.full_name)));

  function toggle(full: string) {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(full)) next.delete(full);
      else next.add(full);
      return next;
    });
  }

  const chosen = repos.filter((r) => selected.has(r.full_name));
  const authorizeHref = `/api/star/authorize?intent=${encodeURIComponent(id)}&repos=${encodeURIComponent(
    chosen.map((r) => r.full_name).join(",")
  )}`;

  return (
    <div className="mt-8">
      <div className="mb-4 flex items-center justify-between text-xs text-muted-foreground">
        <span>{repos.length} repos in this package</span>
        <div className="flex gap-3">
          <button onClick={() => setSelected(new Set(repos.map((r) => r.full_name)))} className="transition-colors hover:text-foreground">
            Select all
          </button>
          <button onClick={() => setSelected(new Set())} className="transition-colors hover:text-foreground">
            Clear
          </button>
        </div>
      </div>

      <ul className="surface divide-y divide-border overflow-hidden p-0">
        {repos.map((r) => (
          <li key={r.full_name} className="flex items-center gap-3 bg-secondary/70 px-4 py-4">
            <input
              type="checkbox"
              checked={selected.has(r.full_name)}
              onChange={() => toggle(r.full_name)}
              className="h-4 w-4 accent-white"
              aria-label={`star ${r.full_name}`}
            />
            <a href={r.html_url} className="flex-1 truncate text-sm text-foreground transition-colors hover:text-muted-foreground">
              {r.full_name}
            </a>
            <span className="text-xs text-muted-foreground">{r.license_spdx || "no license"}</span>
          </li>
        ))}
      </ul>

      <a
        href={chosen.length > 0 ? authorizeHref : undefined}
        aria-disabled={chosen.length === 0}
        className={`mt-7 flex min-h-11 w-full items-center justify-center gap-2 rounded-full px-5 py-3 font-medium transition-transform ${
          chosen.length > 0
            ? "bg-primary text-primary-foreground hover:scale-[1.01]"
            : "cursor-not-allowed bg-muted text-muted-foreground"
        }`}
      >
        Authorize GitHub & star selected ({chosen.length})
      </a>
      <p className="mt-4 text-center text-xs text-muted-foreground">
        No rewards, no unlocking — just credit to maintainers. You&rsquo;ll review GitHub&rsquo;s
        permission screen next.
      </p>
    </div>
  );
}
