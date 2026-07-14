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
/// A reference repo for a package. The webview renders `full_name` + links to
/// `html_url`; license is unknown locally (the account server enriches it for
/// the opt-in star flow), so it's intentionally omitted here.
#[derive(Debug, Clone, Serialize)]
pub struct RepoRef {
    pub full_name: String,
    pub html_url: String,
}
/// A "package" the Hub can activate — compiled from the category's curated
/// conventions (→ a rules markdown) and scaffolds (→ skill recipes). Built
/// LOCALLY from the embedded taxonomy, so activation works with no account
/// server (the account server is only needed for live repo enrichment/starring).
/// Field shape matches exactly what the webview's renderDetail reads
/// (`description`, `counts`, `references[].full_name/html_url`) — review #6/#9/#14.
#[derive(Debug, Clone, Serialize)]
pub struct Package {
    pub slug: String,
    pub name: String,
    pub description: String,
    pub rules: String,
    pub skills: Vec<Skill>,
    pub references: Vec<RepoRef>,
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
    let references: Vec<RepoRef> = c
        .example_repos
        .iter()
        .map(|r| RepoRef {
            full_name: r.clone(),
            html_url: format!("https://github.com/{r}"),
        })
        .collect();
    let counts = Counts {
        rules: c.conventions.len(),
        skills: skills.len(),
    };
    Some(Package {
        slug: c.slug,
        name: c.name,
        description: c.description,
        rules,
        skills,
        references,
        counts,
    })
}

/// Curated intent → keyword expansion, so a vague query still finds the right
/// domains. Returns the extra terms to OR into the query (empty for unknowns).
fn expand(term: &str) -> &'static [&'static str] {
    match term {
        "website" | "web" | "site" | "webpage" | "webapp" | "webapps" | "frontend" | "ui"
        | "html" => &[
            "web",
            "frontend",
            "css",
            "html",
            "react",
            "spa",
            "javascript",
            "fullstack",
        ],
        "ml" | "ai" | "machine" | "learning" | "model" | "models" | "neural" | "deep" | "llm" => &[
            "machine",
            "learning",
            "data",
            "science",
            "deep",
            "nlp",
            "ml",
            "ai",
            "pytorch",
            "tensorflow",
        ],
        "data" | "dataset" | "datasets" | "analytics" | "analysis" | "pipeline" => &[
            "data",
            "science",
            "engineering",
            "analytics",
            "etl",
            "pandas",
        ],
        "game" | "games" | "gaming" | "gamedev" => &[
            "game",
            "development",
            "unity",
            "godot",
            "unreal",
            "rendering",
        ],
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
        | "kubernetes" | "k8s" | "docker" => &[
            "devops",
            "infrastructure",
            "cloud",
            "native",
            "orchestration",
            "ci-cd",
            "pipelines",
        ],
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

/// Conservative singular stem: only strips a trailing "s" on words of length >= 5
/// that don't end in "ss" — so "databases"→"database", "frameworks"→"framework",
/// "models"→"model", while short tech terms (ios, css, js, apis) are left alone.
/// Applied to both query and catalog tokens so plural/singular match (review #5
/// follow-on: a name-substring shouldn't lose to a description-exact match).
fn stem(t: &str) -> String {
    if t.len() >= 5 && t.ends_with('s') && !t.ends_with("ss") {
        t[..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}

fn norm(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        // Keep tokens of length >= 2, plus the single-character language names
        // that are real categories ("c", "r", "d") — dropping them lost valid
        // queries (review #12). Other 1-char tokens stay filtered as noise.
        .filter(|t| t.len() >= 2 || matches!(*t, "c" | "r" | "d"))
        .map(stem)
        .collect()
}

/// Intent-aware fuzzy search. Scores categories by query-term overlap (and
/// expanded-intent overlap) against name/slug/topics/example-repos/description,
/// with weights and a substring fuzzy fallback. Empty query → all categories.
pub fn search(query: &str, limit: usize) -> Vec<Category> {
    use std::collections::{HashMap, HashSet};
    let cats = categories();
    let qterms = norm(query);
    if qterms.is_empty() {
        return cats.into_iter().take(limit).collect();
    }
    // Build the weighted term set DEDUPED, keeping the MAX weight per term. A
    // term that is both a direct query token (1.0) and an expansion (0.5) — or an
    // expansion shared across two query tokens — therefore counts exactly ONCE at
    // its highest weight, instead of being summed multiple times (review #3/#4).
    let mut wmap: HashMap<String, f32> = HashMap::new();
    for t in &qterms {
        let e = wmap.entry(t.clone()).or_insert(0.0);
        *e = e.max(1.0);
    }
    for t in &qterms {
        for ex in expand(t) {
            // Split + stem expansion terms the same way as catalog tokens, so
            // e.g. "databases"/"ci-cd" line up with how categories are tokenized.
            for token in norm(ex) {
                let e = wmap.entry(token).or_insert(0.0);
                *e = e.max(0.5);
            }
        }
    }
    let weighted: Vec<(String, f32)> = wmap.into_iter().collect();
    let qset: HashSet<String> = qterms.iter().cloned().collect();

    // (score, coverage = # distinct DIRECT query terms matched, slug, category)
    let mut scored: Vec<(f32, usize, String, Category)> = cats
        .into_iter()
        .filter_map(|c| {
            let name = norm(&c.name);
            let slug = norm(&c.slug);
            let desc = norm(&c.description);
            let topics: Vec<String> = c.github_topics.iter().flat_map(|t| norm(t)).collect();
            let repos: Vec<String> = c.example_repos.iter().flat_map(|t| norm(t)).collect();
            let mut s = 0.0f32;
            let mut covered: HashSet<&str> = HashSet::new();
            for (qt, w) in &weighted {
                let hit = if name.iter().any(|x| x == qt) {
                    s += 3.0 * w;
                    true
                } else if slug.iter().any(|x| x == qt) {
                    s += 2.5 * w;
                    true
                } else if topics.iter().any(|x| x == qt) {
                    s += 2.0 * w;
                    true
                } else if repos.iter().any(|x| x == qt) {
                    s += 1.5 * w;
                    true
                } else if desc.iter().any(|x| x == qt) {
                    s += 1.0 * w;
                    true
                } else if name
                    .iter()
                    .chain(topics.iter())
                    .any(|x| x.contains(qt.as_str()) || qt.contains(x.as_str()))
                {
                    s += 0.6 * w; // fuzzy: shared substring on a strong field
                    true
                } else if desc.iter().any(|x| x.contains(qt.as_str())) {
                    s += 0.3 * w;
                    true
                } else {
                    false
                };
                if hit && qset.contains(qt.as_str()) {
                    covered.insert(qt.as_str());
                }
            }
            if s > 0.0 {
                Some((s, covered.len(), c.slug.clone(), c))
            } else {
                None
            }
        })
        .collect();
    // Sort: score desc, then coverage (a category matching MORE distinct query
    // terms wins a tie), then slug asc — a STABLE, input-order-independent
    // tie-break so results don't depend on taxonomy ordering luck (review #5).
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.1.cmp(&a.1))
            .then(a.2.cmp(&b.2))
    });
    scored
        .into_iter()
        .take(limit)
        .map(|(_, _, _, c)| c)
        .collect()
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
            r.iter().take(3).any(|c| c.slug.contains("frontend")
                || c.slug.contains("web")
                || c.slug.contains("fullstack")),
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
        for q in [
            "make a game",
            "mobile app",
            "deploy to the cloud",
            "rest api backend",
        ] {
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
        assert!(
            p.counts.rules > 0 || p.counts.skills > 0,
            "package should carry steering"
        );
        assert!(package("nope-not-a-slug").is_none());
    }

    #[test]
    fn package_references_carry_full_name_and_url() {
        let p = package("frontend-spa-frameworks").expect("known slug");
        assert!(!p.references.is_empty());
        let r = &p.references[0];
        assert!(r.full_name.contains('/'), "full_name like owner/repo");
        assert!(r.html_url.starts_with("https://github.com/"));
        assert!(
            !p.description.is_empty(),
            "detail subtitle must not be blank"
        );
    }

    #[test]
    fn scoring_picks_specific_category_not_taxonomy_order() {
        // Single intent token must land its canonical category at the top, not
        // whichever incidental substring match comes first in the taxonomy.
        assert!(search("ios app", 3)
            .iter()
            .take(3)
            .any(|c| c.slug.contains("ios")
                || c.slug.contains("react-native")
                || c.slug.contains("mobile")));
        assert!(search("database", 3)[0].slug.contains("database"));
        assert!(search("computer vision", 3)[0].slug.contains("vision"));
        // The dedupe-max must not let expansions drown the direct match.
        assert!(search("game development", 1)[0].slug.contains("game"));
    }

    #[test]
    fn single_char_language_tokens_survive() {
        // "c" / "r" are real languages — they must not be filtered out as noise.
        assert!(!norm("c").is_empty());
        assert!(!norm("r lang").is_empty());
        assert!(norm("a").is_empty(), "generic single chars stay filtered");
    }
}
