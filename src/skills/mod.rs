use crate::orchestrator::Orchestrator;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

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
            self.create_default_skills().await?;
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

    async fn create_default_skills(&self) -> Result<()> {
        let default_skills = vec![
            Skill {
                name: "docker-expert".to_string(),
                description: "Expert at Docker containerization and orchestration".to_string(),
                version: "1.0.0".to_string(),
                author: Some("Forge Team".to_string()),
                tags: vec!["docker".to_string(), "containers".to_string(), "devops".to_string()],
                prompts: SkillPrompts {
                    system: "You are a Docker expert. Provide optimized Dockerfiles and docker-compose configurations.".to_string(),
                    planning: Some("Analyze the application and suggest optimal Docker architecture.".to_string()),
                    execution: Some("Write production-ready Docker configurations.".to_string()),
                    review: Some("Review Docker configs for security and efficiency.".to_string()),
                },
                settings: SkillSettings {
                    model: Some("qwen2.5-coder:7b".to_string()),
                    temperature: Some(0.5),
                    max_tokens: Some(2048),
                    tools: vec!["docker".to_string()],
                },
                recipes: vec![
                    Recipe {
                        name: "Multi-stage Build".to_string(),
                        description: "Create optimized multi-stage Docker builds".to_string(),
                        trigger_keywords: vec!["docker".to_string(), "containerize".to_string(), "dockerfile".to_string()],
                        steps: vec![
                            RecipeStep {
                                name: "Analyze Requirements".to_string(),
                                prompt_template: "Analyze the application for base image and dependencies".to_string(),
                                expected_output: Some("Base image recommendation".to_string()),
                            },
                        ],
                    },
                ],
            },
            Skill {
                name: "security-auditor".to_string(),
                description: "Comprehensive security auditing for code".to_string(),
                version: "1.0.0".to_string(),
                author: Some("Forge Team".to_string()),
                tags: vec!["security".to_string(), "audit".to_string(), "vulnerabilities".to_string()],
                prompts: SkillPrompts {
                    system: "You are a security expert. Identify vulnerabilities and suggest fixes.".to_string(),
                    planning: Some("Scan code for common security issues.".to_string()),
                    execution: Some("Generate detailed security report.".to_string()),
                    review: None,
                },
                settings: SkillSettings {
                    model: Some("deepseek-coder-v2:16b".to_string()),
                    temperature: Some(0.3),
                    max_tokens: Some(4096),
                    tools: vec!["semgrep".to_string(), "gitleaks".to_string()],
                },
                recipes: vec![],
            },
            Skill {
                name: "react-native-expert".to_string(),
                description: "Build production-ready React Native applications".to_string(),
                version: "1.0.0".to_string(),
                author: Some("Community".to_string()),
                tags: vec!["mobile".to_string(), "react-native".to_string(), "ios".to_string(), "android".to_string()],
                prompts: SkillPrompts {
                    system: "You are a React Native expert. Build performant, cross-platform mobile applications.".to_string(),
                    planning: Some("Plan React Native architecture with proper navigation and state management.".to_string()),
                    execution: Some("Implement components, screens, and native modules.".to_string()),
                    review: Some("Review for performance and platform-specific issues.".to_string()),
                },
                settings: SkillSettings {
                    model: Some("qwen2.5-coder:14b".to_string()),
                    temperature: Some(0.7),
                    max_tokens: Some(4096),
                    tools: vec!["expo".to_string(), "xcode".to_string()],
                },
                recipes: vec![],
            },
            Skill {
                name: "api-designer".to_string(),
                description: "Design RESTful and GraphQL APIs".to_string(),
                version: "1.0.0".to_string(),
                author: Some("Forge Team".to_string()),
                tags: vec!["api".to_string(), "rest".to_string(), "graphql".to_string(), "backend".to_string()],
                prompts: SkillPrompts {
                    system: "You are an API design expert. Create clean, documented APIs following best practices.".to_string(),
                    planning: Some("Design API endpoints and data models.".to_string()),
                    execution: Some("Implement API routes and middleware.".to_string()),
                    review: Some("Review API design for consistency and performance.".to_string()),
                },
                settings: SkillSettings {
                    model: Some("qwen2.5-coder:7b".to_string()),
                    temperature: Some(0.5),
                    max_tokens: Some(2048),
                    tools: vec![],
                },
                recipes: vec![],
            },
        ];
        
        for skill in default_skills {
            let path = self.skills_dir.join(format!("{}.json", skill.name));
            let json = serde_json::to_string_pretty(&skill)?;
            tokio::fs::write(&path, json).await?;
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
            
            if skill.tags.iter().any(|t| t.to_lowercase().contains(&query_lower)) {
                return Some(skill.clone());
            }
            
            for recipe in &skill.recipes {
                if recipe.name.to_lowercase().contains(&query_lower) {
                    return Some(skill.clone());
                }
                if recipe.trigger_keywords.iter().any(|k| query_lower.contains(&k.to_lowercase())) {
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
                        debug!("Matched skill '{}' via recipe '{}'", skill.name, recipe.name);
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
