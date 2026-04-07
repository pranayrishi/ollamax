use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

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

    pub async fn load_skills(&self) -> Result<Vec<Skill>> {
        let mut skills = Vec::new();

        if !self.skills_dir.exists() {
            tokio::fs::create_dir_all(&self.skills_dir).await?;
            // First run: copy the bundled recipes from the binary onto disk so
            // the user can edit them. We `include_str!` rather than relying on
            // a runtime path because the binary may be installed anywhere.
            self.write_bundled_recipes().await?;
        }

        let mut entries = tokio::fs::read_dir(&self.skills_dir).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                if let Ok(content) = tokio::fs::read_to_string(&path).await {
                    if let Ok(skill) = serde_json::from_str::<Skill>(&content) {
                        skills.push(skill.clone());
                        let mut skills_map = self.skills.write().await;
                        skills_map.insert(skill.name.clone(), skill);
                    }
                }
            }
        }

        info!("Loaded {} skills", skills.len());
        Ok(skills)
    }

    /// Write the bundled recipes (the actual `skills/recipes/*.json` files in
    /// the repo) to the user's skills directory on first run.
    ///
    /// Previously this was a `create_default_skills` function that
    /// hand-rolled stripped-down versions of each skill in Rust source — they
    /// drifted out of sync with the real bundled JSONs (e.g., docker-expert
    /// had only 1 recipe step instead of the full multi-stage workflow). Now
    /// the real JSONs are baked in via `include_str!` and there is exactly
    /// one source of truth.
    async fn write_bundled_recipes(&self) -> Result<()> {
        for (filename, contents) in BUNDLED_RECIPES {
            let path = self.skills_dir.join(filename);
            tokio::fs::write(&path, contents).await?;
        }
        Ok(())
    }

    pub async fn find_skill(&self, query: &str) -> Option<Skill> {
        let skills = self.skills.read().await;
        let query_lower = query.to_lowercase();

        for skill in skills.values() {
            if skill.name.to_lowercase().contains(&query_lower) {
                return Some(skill.clone());
            }

            if skill
                .tags
                .iter()
                .any(|t| t.to_lowercase().contains(&query_lower))
            {
                return Some(skill.clone());
            }

            for recipe in &skill.recipes {
                if recipe.name.to_lowercase().contains(&query_lower) {
                    return Some(skill.clone());
                }
                if recipe
                    .trigger_keywords
                    .iter()
                    .any(|k| query_lower.contains(&k.to_lowercase()))
                {
                    return Some(skill.clone());
                }
            }
        }

        None
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
