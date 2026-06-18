// Server-side catalog ingestion. Discovers domain repos via the GitHub Search
// API (there is no official "trending" API), applies the quality/license gates,
// and upserts metadata. Clients NEVER call GitHub — they read our cached
// catalog. Run on a schedule (see /api/hub/refresh) so packages stay current.
//
// Rate-limit posture: an authenticated `GITHUB_INGEST_TOKEN` gives 5,000 req/hr
// (30 search req/min); we cap queries per category, page small, and pace
// requests. Without a token it falls back to 60/hr (and stops early on 403).
import "server-only";
import { upsertHubRepo, mapRepoToCategory, type HubRepo } from "@/lib/db";
import { CATEGORIES, type Category } from "./taxonomy";
import { qualityScore, passesGate } from "./quality";

const GH = "https://api.github.com";

function ghHeaders(): Record<string, string> {
  const h: Record<string, string> = {
    Accept: "application/vnd.github+json",
    "User-Agent": "ollama-forge-hub-ingest",
    "X-GitHub-Api-Version": "2022-11-28",
  };
  const t = process.env.GITHUB_INGEST_TOKEN;
  if (t) h.Authorization = `Bearer ${t}`;
  return h;
}

type SearchItem = {
  full_name: string;
  description: string | null;
  stargazers_count: number;
  forks_count: number;
  language: string | null;
  topics?: string[];
  html_url: string;
  pushed_at: string | null;
  license?: { spdx_id?: string; name?: string } | null;
};

function toHubRepo(item: SearchItem): HubRepo {
  const spdx =
    item.license?.spdx_id && item.license.spdx_id !== "NOASSERTION"
      ? item.license.spdx_id
      : null;
  const r: HubRepo = {
    full_name: item.full_name,
    description: item.description ?? null,
    stars: item.stargazers_count ?? 0,
    forks: item.forks_count ?? 0,
    language: item.language ?? null,
    topics: item.topics ?? [],
    license_spdx: spdx,
    license_name: item.license?.name ?? null,
    html_url: item.html_url,
    pushed_at: item.pushed_at ?? null,
    quality_score: 0,
    included: false,
  };
  r.quality_score = qualityScore(r);
  r.included = passesGate(r).included;
  return r;
}

export async function ingestCategory(
  cat: Category
): Promise<{ category: string; found: number; included: number; rateLimited: boolean }> {
  let found = 0;
  let included = 0;
  let rateLimited = false;
  for (const q of cat.searchQueries.slice(0, 3)) {
    const url = `${GH}/search/repositories?q=${encodeURIComponent(q)}&sort=stars&order=desc&per_page=20`;
    let res: Response;
    try {
      res = await fetch(url, { headers: ghHeaders() });
    } catch {
      continue;
    }
    if (res.status === 403 || res.status === 429) {
      rateLimited = true;
      break; // back off; the scheduled run will resume later
    }
    if (!res.ok) continue;
    const body = (await res.json()) as { items?: SearchItem[] };
    for (const item of body.items ?? []) {
      const repo = toHubRepo(item);
      await upsertHubRepo(repo);
      await mapRepoToCategory(cat.slug, repo.full_name);
      found++;
      if (repo.included) included++;
    }
    await new Promise((r) => setTimeout(r, 800)); // pace search requests
  }
  return { category: cat.slug, found, included, rateLimited };
}

export async function ingestAll(
  limit?: number
): Promise<{ category: string; found: number; included: number; rateLimited: boolean }[]> {
  const cats = limit ? CATEGORIES.slice(0, limit) : CATEGORIES;
  const results = [];
  for (const c of cats) {
    const r = await ingestCategory(c);
    results.push(r);
    if (r.rateLimited) break; // stop the whole run if we hit the ceiling
  }
  return results;
}
