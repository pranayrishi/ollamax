//! Smoke test: every JSON file in `skills/recipes/` must deserialize into a
//! `Skill`. This catches the boring class of "shipped a skill, forgot a comma,
//! everyone's CLI now panics on `forge skills list`" bugs.

use ollama_forge::skills::Skill;
use std::fs;
use std::path::PathBuf;

#[test]
fn all_bundled_recipes_parse() {
    let recipes_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("skills/recipes");
    assert!(
        recipes_dir.is_dir(),
        "skills/recipes/ missing — bundled recipes are part of the shipped artifact"
    );

    let mut checked = 0usize;
    for entry in fs::read_dir(&recipes_dir).expect("read skills/recipes/") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let raw = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let skill: Skill = serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
        assert!(!skill.name.is_empty(), "{}: empty name", path.display());
        assert!(
            !skill.description.is_empty(),
            "{}: empty description",
            path.display()
        );
        checked += 1;
    }
    assert!(checked > 0, "no bundled recipes were checked");
}
