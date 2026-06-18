use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

/// Anthropic-style SKILL.md frontmatter — the format Claude Code reads.
/// We accept it so a user can drop in any skill from
/// `https://github.com/anthropics/skills` (or fork it) and it works.
///
/// Spec (de-facto, observed across Claude Code releases):
///
/// ```yaml
/// ---
/// name: my-skill
/// description: One-line summary used for matching against the user task.
/// model: optional-model-hint                  # forge-specific extension
/// temperature: 0.5                             # forge-specific extension
/// tags: [optional, list, of, tags]
/// ---
/// # Markdown body...
/// ```
///
/// The Markdown body becomes the system prompt verbatim.
#[derive(Debug, Deserialize)]
struct SkillFrontmatter {
    name: String,
    description: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    temperature: Option<f32>,
    #[serde(default)]
    max_tokens: Option<usize>,
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    author: Option<String>,
}

/// Parse an Anthropic-style SKILL.md (YAML frontmatter + markdown body) into
/// our native `Skill` shape. Strict on required fields, lenient on optional
/// ones — we want existing Claude Code skills to load with zero edits.
pub fn parse_skill_md(content: &str) -> Result<Skill> {
    let body = content.trim_start();
    if !body.starts_with("---") {
        return Err(anyhow!(
            "SKILL.md must start with `---` YAML frontmatter delimiter"
        ));
    }
    // Find the closing delimiter. The first line is `---\n`; look for the
    // next `---` on its own line.
    let after_open = &body[3..];
    let close_offset = after_open
        .find("\n---")
        .ok_or_else(|| anyhow!("SKILL.md frontmatter is unterminated (no closing `---`)"))?;
    let frontmatter_yaml = &after_open[..close_offset];
    let body_md = after_open[close_offset + 4..].trim_start_matches('\n');

    let fm: SkillFrontmatter =
        serde_yaml::from_str(frontmatter_yaml).context("parse SKILL.md YAML frontmatter")?;

    Ok(Skill {
        name: fm.name,
        description: fm.description,
        version: fm.version.unwrap_or_else(|| "1.0.0".to_string()),
        author: fm.author,
        tags: fm.tags,
        prompts: SkillPrompts {
            // The markdown body is the system prompt. This matches how
            // Claude Code uses SKILL.md files.
            system: body_md.trim().to_string(),
            planning: None,
            execution: None,
            review: None,
        },
        settings: SkillSettings {
            model: fm.model,
            temperature: fm.temperature,
            max_tokens: fm.max_tokens,
            tools: fm.tools,
        },
        recipes: Vec::new(),
    })
}

/// `(filename, raw JSON)` pairs baked into the binary at compile time. The
/// JSON itself lives in `skills/recipes/*.json` so it stays diff-friendly,
/// gets validated by `tests/skill_recipes_parse.rs`, and stays in sync with
/// what gets shipped to users on first run.
const BUNDLED_RECIPES: &[(&str, &str)] = &[
    (
        "docker-expert.json",
        include_str!("../../skills/recipes/docker-expert.json"),
    ),
    (
        "security-auditor.json",
        include_str!("../../skills/recipes/security-auditor.json"),
    ),
    (
        "react-native-expert.json",
        include_str!("../../skills/recipes/react-native-expert.json"),
    ),
    (
        "api-designer.json",
        include_str!("../../skills/recipes/api-designer.json"),
    ),
    (
        "lora-finetune.json",
        include_str!("../../skills/recipes/lora-finetune.json"),
    ),
];

pub struct SkillsEngine {
    skills: Arc<RwLock<HashMap<String, Skill>>>,
    skills_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub version: String,
    pub author: Option<String>,
    pub tags: Vec<String>,
    pub prompts: SkillPrompts,
    pub settings: SkillSettings,
    pub recipes: Vec<Recipe>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillPrompts {
    pub system: String,
    pub planning: Option<String>,
    pub execution: Option<String>,
    pub review: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillSettings {
    pub model: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<usize>,
    pub tools: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recipe {
    pub name: String,
    pub description: String,
    pub trigger_keywords: Vec<String>,
    pub steps: Vec<RecipeStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeStep {
    pub name: String,
    pub prompt_template: String,
    pub expected_output: Option<String>,
}

impl SkillsEngine {
    pub fn new(skills_dir: PathBuf) -> Self {
        Self {
            skills: Arc::new(RwLock::new(HashMap::new())),
            skills_dir,
        }
    }

    /// Pick the single loaded skill whose name+description best matches the
    /// task, or None if nothing clears a small relevance bar. Lightweight token
    /// overlap (same spirit as hub search) — enough to AUTO-APPLY one relevant
    /// skill's guidance into the agent's system prompt (Hermes-class
    /// "skills in the loop"). Call after `load_skills`.
    pub async fn best_match(&self, query: &str) -> Option<Skill> {
        fn toks(s: &str) -> Vec<String> {
            s.to_lowercase()
                .split(|c: char| !c.is_alphanumeric())
                .filter(|t| t.len() >= 4)
                .map(|t| t.to_string())
                .collect()
        }
        let q = toks(query);
        if q.is_empty() {
            return None;
        }
        let skills = self.skills.read().await;
        let mut best: Option<(usize, Skill)> = None;
        for s in skills.values() {
            let hay = toks(&format!("{} {}", s.name, s.description));
            let score = q.iter().filter(|t| hay.contains(*t)).count();
            if score > 0 && best.as_ref().map_or(true, |(b, _)| score > *b) {
                best = Some((score, s.clone()));
            }
        }
        best.map(|(_, s)| s)
    }

    pub async fn load_skills(&self) -> Result<Vec<Skill>> {
        let mut skills = Vec::new();

        if !self.skills_dir.exists() {
            tokio::fs::create_dir_all(&self.skills_dir).await?;
        }
        // Always sync any *new* bundled recipes that don't yet exist on disk.
        // The previous version only wrote bundled recipes on first run, so
        // existing users never got new recipes (e.g., `lora-finetune` shipped
        // in session 7) without manually deleting their skills dir. This
        // sync only adds — it never overwrites a recipe the user has edited.
        self.sync_bundled_recipes().await?;

        // Walk the dir non-recursively. Two formats are accepted:
        //   1. `*.json` — forge-native skill JSON.
        //   2. `*/SKILL.md` or `<name>.SKILL.md` — Anthropic SKILL.md format,
        //      a Markdown file with YAML frontmatter. We convert on-the-fly
        //      so users can drop in skills authored for Claude Code.
        let mut entries = tokio::fs::read_dir(&self.skills_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            // Recurse one level into subdirectories looking for SKILL.md.
            if entry.file_type().await?.is_dir() {
                let candidate = path.join("SKILL.md");
                if candidate.exists() {
                    if let Ok(content) = tokio::fs::read_to_string(&candidate).await {
                        match parse_skill_md(&content) {
                            Ok(skill) => {
                                skills.push(skill.clone());
                                self.skills.write().await.insert(skill.name.clone(), skill);
                            }
                            Err(e) => debug!("skipping {}: {e}", candidate.display()),
                        }
                    }
                }
                continue;
            }

            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let name = path.file_name().and_then(|e| e.to_str()).unwrap_or("");

            if ext == "json" {
                if let Ok(content) = tokio::fs::read_to_string(&path).await {
                    if let Ok(skill) = serde_json::from_str::<Skill>(&content) {
                        skills.push(skill.clone());
                        self.skills.write().await.insert(skill.name.clone(), skill);
                    }
                }
            } else if name.ends_with(".SKILL.md") || name == "SKILL.md" {
                if let Ok(content) = tokio::fs::read_to_string(&path).await {
                    match parse_skill_md(&content) {
                        Ok(skill) => {
                            skills.push(skill.clone());
                            self.skills.write().await.insert(skill.name.clone(), skill);
                        }
                        Err(e) => debug!("skipping {}: {e}", path.display()),
                    }
                }
            }
        }

        info!("Loaded {} skills", skills.len());
        Ok(skills)
    }

    /// Sync any bundled recipes that don't yet exist on disk into the
    /// user's skills directory. Add-only — never overwrites a file the
    /// user may have edited. This is what gives existing users access
    /// to new bundled recipes (e.g., `lora-finetune` added in session 7)
    /// without forcing them to delete their skills dir.
    ///
    /// Idempotent: safe to call on every `load_skills`. The cost is one
    /// `Path::exists` syscall per bundled recipe, which is negligible.
    async fn sync_bundled_recipes(&self) -> Result<()> {
        for (filename, contents) in BUNDLED_RECIPES {
            let path = self.skills_dir.join(filename);
            if !path.exists() {
                tokio::fs::write(&path, contents).await?;
                debug!("synced bundled recipe to {}", path.display());
            }
        }
        Ok(())
    }

    /// Find a single skill by name/tag/keyword. Returns the *first* match,
    /// useful for `forge run-skill` where the user typed an exact name.
    pub async fn find_skill(&self, query: &str) -> Option<Skill> {
        self.find_all_matching(query).await.into_iter().next()
    }

    /// All skills matching `query` by name, tag, recipe name, or trigger
    /// keyword. Used by `forge skills search` so the user sees every option
    /// when they type a vague term like "react".
    pub async fn find_all_matching(&self, query: &str) -> Vec<Skill> {
        let skills = self.skills.read().await;
        let q = query.to_lowercase();
        let mut out = Vec::new();
        for skill in skills.values() {
            let name_hit = skill.name.to_lowercase().contains(&q);
            let tag_hit = skill.tags.iter().any(|t| t.to_lowercase().contains(&q));
            let recipe_hit = skill.recipes.iter().any(|r| {
                r.name.to_lowercase().contains(&q)
                    || r.trigger_keywords
                        .iter()
                        .any(|k| q.contains(&k.to_lowercase()))
            });
            if name_hit || tag_hit || recipe_hit {
                out.push(skill.clone());
            }
        }
        out
    }

    pub async fn add_skill(&self, skill: Skill) -> Result<()> {
        let path = self.skills_dir.join(format!("{}.json", skill.name));
        let json = serde_json::to_string_pretty(&skill)?;
        tokio::fs::write(&path, json).await?;

        let mut skills = self.skills.write().await;
        skills.insert(skill.name.clone(), skill);

        Ok(())
    }

    pub async fn remove_skill(&self, name: &str) -> Result<()> {
        let path = self.skills_dir.join(format!("{}.json", name));
        if path.exists() {
            tokio::fs::remove_file(&path).await?;
        }

        let mut skills = self.skills.write().await;
        skills.remove(name);

        Ok(())
    }

    pub async fn list_skills(&self) -> Vec<Skill> {
        let skills = self.skills.read().await;
        skills.values().cloned().collect()
    }

    pub async fn match_skill_to_task(&self, task: &str) -> Option<Skill> {
        let task_lower = task.to_lowercase();

        let skills = self.skills.read().await;

        for skill in skills.values() {
            for recipe in &skill.recipes {
                for keyword in &recipe.trigger_keywords {
                    if task_lower.contains(&keyword.to_lowercase()) {
                        debug!(
                            "Matched skill '{}' via recipe '{}'",
                            skill.name, recipe.name
                        );
                        return Some(skill.clone());
                    }
                }
            }

            for tag in &skill.tags {
                if task_lower.contains(&tag.to_lowercase()) {
                    return Some(skill.clone());
                }
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn best_match_picks_relevant_skill() {
        let dir = std::env::temp_dir().join(format!("forge-skillmatch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("alpha.SKILL.md"),
            "---\nname: alpha-skill\ndescription: Handles zqxwvtoken alpha workflows specially\n---\nbody",
        )
        .unwrap();
        let eng = SkillsEngine::new(dir.clone());
        eng.load_skills().await.unwrap();
        let m = eng.best_match("please use the zqxwvtoken approach here").await;
        assert_eq!(m.map(|s| s.name), Some("alpha-skill".to_string()));
        // A query sharing no >=4-char token with any skill matches nothing.
        assert!(eng.best_match("qqqzzz wwwyyy").await.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
