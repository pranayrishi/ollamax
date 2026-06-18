// Compile a category into a "package": the transparent steering a client
// applies. A package is NOT whole repos dumped into context — it's:
//   - rules:      generic, license-safe domain conventions (→ rules_suffix)
//   - skills:     scaffold recipes in forge's native Skill JSON shape (→ SkillsEngine)
//   - references: curated LINKS to gated repos, with license + attribution
//   - routingHints: a domain tag
// The client writes rules/skills into the user's config dirs so the existing
// steering mechanisms pick them up. Honest: it's steering, not magic.
import "server-only";
import { getCategory } from "./taxonomy";
import { getCategoryRepos } from "@/lib/db";
import { isPermissive } from "./quality";

export type CompiledPackage = {
  slug: string;
  name: string;
  description: string;
  rules: string; // markdown for the rules dir
  skills: unknown[]; // forge Skill JSON objects
  references: {
    full_name: string;
    html_url: string;
    stars: number;
    license: string | null;
    permissive: boolean;
    description: string | null;
  }[];
  routingHints: { domain: string };
  counts: { rules: number; skills: number; references: number };
};

export async function compilePackage(slug: string): Promise<CompiledPackage | null> {
  const cat = getCategory(slug);
  if (!cat) return null;
  const repos = await getCategoryRepos(slug, true);

  const rules = renderRules(cat.name, cat.conventions);
  const skills = (cat.scaffolds ?? []).map((s, i) => skillRecipe(slug, cat.name, cat.githubTopics, s, i));
  const references = repos.map((r) => ({
    full_name: r.full_name,
    html_url: r.html_url,
    stars: r.stars,
    license: r.license_spdx,
    permissive: isPermissive(r.license_spdx),
    description: r.description,
  }));

  return {
    slug: cat.slug,
    name: cat.name,
    description: cat.description,
    rules,
    skills,
    references,
    routingHints: { domain: cat.slug },
    counts: { rules: cat.conventions.length, skills: skills.length, references: references.length },
  };
}

function renderRules(name: string, conventions: string[]): string {
  let md = `# ${name} — domain best-practice rules\n\n`;
  md +=
    `_Activated from the Ollama-Forge Hub. Transparent steering: these are generic, ` +
    `license-safe conventions — not copied source code. Edit or delete this file to remove them._\n\n`;
  for (const c of conventions) md += `- ${c}\n`;
  return md;
}

// Matches forge's native Skill JSON (see src/skills/mod.rs): name/description/
// version/author/tags/prompts/settings/recipes.
function skillRecipe(slug: string, name: string, topics: string[], scaffold: string, i: number) {
  return {
    name: `hub-${slug}-${i}`,
    description: `${name}: ${scaffold}`,
    version: "1.0.0",
    author: "Ollama-Forge Hub",
    tags: [slug, "hub", ...topics.slice(0, 4)],
    prompts: {
      system:
        `You are an expert in ${name}. Task template: ${scaffold}. ` +
        `Follow domain best practices and produce idiomatic, production-quality output.`,
      planning: null,
      execution: null,
      review: null,
    },
    settings: { model: null, temperature: 0.4, max_tokens: null, tools: [] },
    recipes: [],
  };
}
