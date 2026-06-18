//! Central Hub catalog — served by the LOCAL engine (`forge serve`).
//!
//! ## Fix for #7 (catalog dead-end)
//!
//! The Hub catalog used to live ONLY on the website account server, so the app
//! couldn't load it unless the user set the obscure `forge.accountServer`
//! setting → the "Set forge.accountServer to load the Hub catalog" dead-end.
//! Now the 54-category taxonomy is **embedded in the engine at compile time** and
//! served from `/api/hub/categories`, so the catalog **auto-loads** with zero
//! configuration. (The account server is still used, when present, to enrich a
//! package with the live curated repo list + the opt-in starring flow.)
//!
//! ## Intent-aware search
//!
//! [`search`] is fuzzy + intent-expanding, so loose queries like "build a
//! website" or "ml stuff" return sensible categories instead of a "no matching
//! categories" dead-end — no exact keywords required.

use serde::{Deserialize, Serialize};

const TAXONOMY: &str = include_str!("taxonomy.json");

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Category {
    pub slug: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, rename = "githubTopics")]
    pub github_topics: Vec<String>,
    #[serde(default, rename = "exampleRepos")]
    pub example_repos: Vec<String>,
    #[serde(default)]
    pub conventions: Vec<String>,
    #[serde(default)]
    pub scaffolds: Vec<String>,
}

/// All curated categories (parsed from the embedded taxonomy).
pub fn categories() -> Vec<Category> {
    serde_json::from_str(TAXONOMY).unwrap_or_default()
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillPrompts {
    pub system: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub prompts: SkillPrompts,
}
#[derive(Debug, Clone, Serialize)]
pub struct Counts {
    pub rules: usize,
    pub skills: usize,
}
/// A "package" the Hub can activate — compiled from the category's curated
/// conventions (→ a rules markdown) and scaffolds (→ skill recipes). Built
/// LOCALLY from the embedded taxonomy, so activation works with no account
/// server (the account server is only needed for live repo enrichment/starring).
#[derive(Debug, Clone, Serialize)]
pub struct Package {
    pub slug: String,
    pub name: String,
    pub rules: String,
    pub skills: Vec<Skill>,
    pub references: Vec<String>,
    pub counts: Counts,
}

pub fn package(slug: &str) -> Option<Package> {
    let c = categories().into_iter().find(|c| c.slug == slug)?;
    let mut rules = format!("# {} — best practices\n\n{}\n", c.name, c.description);
    if !c.conventions.is_empty() {
        rules.push_str("\n## Conventions to follow\n");
        for conv in &c.conventions {
            rules.push_str(&format!("- {conv}\n"));
        }
    }
    let skills: Vec<Skill> = c
        .scaffolds
        .iter()
        .enumerate()
        .map(|(i, s)| Skill {
            name: format!("{}-scaffold-{}", c.slug, i + 1),
            description: s.clone(),
            prompts: SkillPrompts {
                system: format!(
                    "You are scaffolding for the {} domain. {} Follow the domain conventions.",
                    c.name, s
                ),
            },
        })
        .collect();
    let counts = Counts { rules: c.conventions.len(), skills: skills.len() };
    Some(Package {
        slug: c.slug,
        name: c.name,
        rules,
        skills,
        references: c.example_repos,
        counts,
    })
}

/// Curated intent → keyword expansion, so a vague query still finds the right
/// domains. Returns the extra terms to OR into the query (empty for unknowns).
fn expand(term: &str) -> &'static [&'static str] {
    match term {
        "website" | "web" | "site" | "webpage" | "webapp" | "webapps" | "frontend" | "ui" | "html" => {
            &["web", "frontend", "css", "html", "react", "spa", "javascript", "fullstack"]
        }
        "ml" | "ai" | "machine" | "learning" | "model" | "models" | "neural" | "deep" | "llm" => {
            &["machine", "learning", "data", "science", "deep", "nlp", "ml", "ai", "pytorch", "tensorflow"]
        }
        "data" | "dataset" | "datasets" | "analytics" | "analysis" | "pipeline" => {
            &["data", "science", "engineering", "analytics", "etl", "pandas"]
        }
        "game" | "games" | "gaming" | "gamedev" => {
            &["game", "development", "unity", "godot", "unreal", "rendering"]
        }
        "app" | "apps" | "mobile" | "ios" | "android" | "phone" => {
            &["mobile", "ios", "android", "react", "native", "flutter"]
        }
        "api" | "apis" | "backend" | "server" | "microservice" | "microservices" => {
            &["backend", "apis", "server", "rest", "graphql", "databases"]
        }
        "3d" | "modeling" | "modelling" | "graphics" | "render" | "rendering" => {
            &["3d", "graphics", "modeling", "rendering", "real-time"]
        }
        "devops" | "infra" | "infrastructure" | "deploy" | "deployment" | "ci" | "cd" | "cloud"
        | "kubernetes" | "k8s" | "docker" => {
            &["devops", "infrastructure", "cloud", "native", "orchestration", "ci-cd", "pipelines"]
        }
        "security" | "sec" | "infosec" | "pentest" | "auth" | "authentication" => {
            &["security", "application", "identity", "auth"]
        }
        "blockchain" | "crypto" | "web3" | "smart" | "contracts" => {
            &["blockchain", "smart", "contracts"]
        }
        "embedded" | "iot" | "firmware" | "hardware" | "robot" | "robotics" => {
            &["embedded", "iot", "robotics"]
        }
        "desktop" | "electron" => &["desktop", "electron", "native"],
        "test" | "testing" | "qa" => &["testing", "qa", "automation"],
        _ => &[],
    }
}

fn norm(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 2)
        .map(String::from)
        .collect()
}

/// Intent-aware fuzzy search. Scores categories by query-term overlap (and
/// expanded-intent overlap) against name/slug/topics/example-repos/description,
/// with weights and a substring fuzzy fallback. Empty query → all categories.
pub fn search(query: &str, limit: usize) -> Vec<Category> {
    let cats = categories();
    let qterms = norm(query);
    if qterms.is_empty() {
        return cats.into_iter().take(limit).collect();
    }
    // Direct query terms (weight 1.0) + intent expansions (weight 0.5).
    let mut weighted: Vec<(String, f32)> = qterms.iter().map(|t| (t.clone(), 1.0)).collect();
    for t in &qterms {
        for e in expand(t) {
            weighted.push((e.to_string(), 0.5));
        }
    }

    let mut scored: Vec<(f32, Category)> = cats
        .into_iter()
        .filter_map(|c| {
            let name = norm(&c.name);
            let slug = norm(&c.slug);
            let desc = norm(&c.description);
            let topics: Vec<String> = c.github_topics.iter().flat_map(|t| norm(t)).collect();
            let repos: Vec<String> = c.example_repos.iter().flat_map(|t| norm(t)).collect();
            let mut s = 0.0f32;
            for (qt, w) in &weighted {
                if name.iter().any(|x| x == qt) {
                    s += 3.0 * w;
                } else if slug.iter().any(|x| x == qt) {
                    s += 2.5 * w;
                } else if topics.iter().any(|x| x == qt) {
                    s += 2.0 * w;
                } else if repos.iter().any(|x| x == qt) {
                    s += 1.5 * w;
                } else if desc.iter().any(|x| x == qt) {
                    s += 1.0 * w;
                } else if name
                    .iter()
                    .chain(topics.iter())
                    .any(|x| x.contains(qt.as_str()) || qt.contains(x.as_str()))
                {
                    s += 0.6 * w; // fuzzy: shared substring on a strong field
                } else if desc.iter().any(|x| x.contains(qt.as_str())) {
                    s += 0.3 * w;
                }
            }
            if s > 0.0 {
                Some((s, c))
            } else {
                None
            }
        })
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().take(limit).map(|(_, c)| c).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taxonomy_loads_all_categories() {
        assert_eq!(categories().len(), 54);
    }

    #[test]
    fn loose_website_query_returns_web_categories() {
        let r = search("build a website", 5);
        assert!(!r.is_empty(), "loose intent query must not dead-end");
        // The top hits should be web/frontend domains.
        assert!(
            r.iter()
                .take(3)
                .any(|c| c.slug.contains("frontend") || c.slug.contains("web") || c.slug.contains("fullstack")),
            "got: {:?}",
            r.iter().map(|c| &c.slug).collect::<Vec<_>>()
        );
    }

    #[test]
    fn loose_ml_query_returns_ml_categories() {
        let r = search("ml stuff", 5);
        assert!(!r.is_empty());
        assert!(
            r.iter().take(4).any(|c| c.slug.contains("ml")
                || c.slug.contains("deep-learning")
                || c.slug.contains("data-science")
                || c.slug.contains("nlp")),
            "got: {:?}",
            r.iter().map(|c| &c.slug).collect::<Vec<_>>()
        );
    }

    #[test]
    fn intent_queries_dont_dead_end() {
        for q in ["make a game", "mobile app", "deploy to the cloud", "rest api backend"] {
            assert!(!search(q, 5).is_empty(), "`{q}` should return categories");
        }
    }

    #[test]
    fn exact_name_ranks_first() {
        let r = search("game development", 3);
        assert!(r[0].slug.contains("game"));
    }

    #[test]
    fn empty_query_returns_catalog() {
        assert!(!search("", 10).is_empty());
    }

    #[test]
    fn package_compiles_rules_and_skills_locally() {
        let p = package("game-development").expect("known slug");
        assert_eq!(p.slug, "game-development");
        assert!(p.rules.contains("best practices"));
        assert!(p.counts.rules > 0 || p.counts.skills > 0, "package should carry steering");
        assert!(package("nope-not-a-slug").is_none());
    }
}
