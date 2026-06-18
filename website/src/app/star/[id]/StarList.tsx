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
      <div className="mb-3 flex items-center justify-between text-xs text-zinc-500">
        <span>{repos.length} repos in this package</span>
        <div className="flex gap-3">
          <button onClick={() => setSelected(new Set(repos.map((r) => r.full_name)))} className="hover:text-zinc-200">
            Select all
          </button>
          <button onClick={() => setSelected(new Set())} className="hover:text-zinc-200">
            Clear
          </button>
        </div>
      </div>

      <ul className="divide-y divide-ink-700/70 overflow-hidden rounded-2xl border border-ink-700">
        {repos.map((r) => (
          <li key={r.full_name} className="flex items-center gap-3 bg-ink-900/50 px-4 py-3">
            <input
              type="checkbox"
              checked={selected.has(r.full_name)}
              onChange={() => toggle(r.full_name)}
              className="h-4 w-4 accent-ember-500"
              aria-label={`star ${r.full_name}`}
            />
            <a href={r.html_url} className="flex-1 truncate text-sm text-zinc-200 hover:text-ember-400">
              {r.full_name}
            </a>
            <span className="text-xs text-zinc-500">{r.license_spdx || "no license"}</span>
          </li>
        ))}
      </ul>

      <a
        href={chosen.length > 0 ? authorizeHref : undefined}
        aria-disabled={chosen.length === 0}
        className={`mt-6 flex w-full items-center justify-center gap-2 rounded-xl px-5 py-3 font-semibold ${
          chosen.length > 0
            ? "bg-ember-500 text-ink-950 hover:bg-ember-400"
            : "cursor-not-allowed bg-ink-800 text-zinc-500"
        }`}
      >
        Authorize GitHub & star selected ({chosen.length})
      </a>
      <p className="mt-3 text-center text-xs text-zinc-600">
        No rewards, no unlocking — just credit to maintainers. You&rsquo;ll review GitHub&rsquo;s
        permission screen next.
      </p>
    </div>
  );
}
