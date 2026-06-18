// Data-driven category taxonomy. Categories live in
// src/data/hub-taxonomy.json (54 seeded from GitHub topics by a curation pass);
// adding/editing a category needs NO code change. Each carries the GitHub
// topics + search queries used to ingest its repos and the generic
// best-practice conventions/scaffolds that compile into package rules/skills.
import taxonomy from "@/data/hub-taxonomy.json";

export type Category = {
  slug: string;
  name: string;
  description: string;
  githubTopics: string[];
  searchQueries: string[];
  exampleRepos?: string[];
  conventions: string[];
  scaffolds?: string[];
};

export const CATEGORIES = taxonomy as unknown as Category[];

export function getCategory(slug: string): Category | undefined {
  return CATEGORIES.find((c) => c.slug === slug);
}
