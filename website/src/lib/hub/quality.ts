// Quality / safety / license gates for catalog curation.
//
// We do NOT trust star count alone — stars are heavily gamed and high-star
// repos are sometimes malware. A repo must clear a gate (license present, real
// adoption, recent maintenance, sane fork ratio, not denylisted) before it's
// "included" in a package. Everything else can still be LINKED but isn't
// presented as a curated reference.
import "server-only";
import type { HubRepo } from "@/lib/db";

// SPDX ids that are permissive enough to reference content beyond a link.
// Copyleft (GPL/AGPL/etc.) repos are still LINKED + attributed, but not used as
// reference snippets — only the link is surfaced.
const PERMISSIVE = new Set([
  "MIT",
  "Apache-2.0",
  "BSD-2-Clause",
  "BSD-3-Clause",
  "ISC",
  "0BSD",
  "MPL-2.0",
  "Unlicense",
  "CC0-1.0",
  "Zlib",
  "BSL-1.0",
]);

// Explicit repo denylist (e.g. known-malicious or unwanted) — full_name form.
const DENYLIST = new Set<string>([]);

export function isPermissive(spdx: string | null): boolean {
  return !!spdx && PERMISSIVE.has(spdx);
}

function ageDays(pushedAt: string | null): number {
  if (!pushedAt) return 99999;
  return (Date.now() - new Date(pushedAt).getTime()) / 86_400_000;
}

/** 0..1 composite — popularity (log) + recency + fork-ratio sanity + license. */
export function qualityScore(r: HubRepo): number {
  const stars = Math.max(0, r.stars);
  const forks = Math.max(0, r.forks);
  const age = ageDays(r.pushed_at);
  const recency = age < 90 ? 1 : age < 365 ? 0.6 : age < 730 ? 0.3 : 0.05;
  const forkRatio = stars > 0 ? forks / stars : 0;
  // Some forks are healthy; an absurdly low fork ratio on very high stars is a
  // classic star-gaming signal.
  const forkSanity = stars < 500 ? 0.7 : forkRatio >= 0.02 ? 1 : 0.4;
  const licenseBonus = r.license_spdx ? 1 : 0.3;
  const popularity = Math.min(1, Math.log10(stars + 10) / 5);
  const score = popularity * 0.4 + recency * 0.3 + forkSanity * 0.2 + licenseBonus * 0.1;
  return Math.round(score * 1000) / 1000;
}

/** Hard gate for inclusion in a package (beyond a bare link). */
export function passesGate(r: HubRepo): { included: boolean; reason: string } {
  if (DENYLIST.has(r.full_name)) return { included: false, reason: "denylisted" };
  if (!r.license_spdx) return { included: false, reason: "no license (all rights reserved) — link only" };
  if (r.stars < 200) return { included: false, reason: "insufficient adoption (<200 stars)" };
  if (ageDays(r.pushed_at) > 1095) return { included: false, reason: "unmaintained (>3y since last push)" };
  return { included: true, reason: "ok" };
}
