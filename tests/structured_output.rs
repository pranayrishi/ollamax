//! Integration test for schema-constrained output.
//!
//! This is the local-LLM equivalent of OpenAI's `response_format` and the
//! closest thing forge currently has to "guaranteed-valid tool calls". The
//! contract: when you pass a JSON Schema as `format`, Ollama's constrained
//! decoder must produce a JSON object that parses against that schema.
//!
//! Gated by `FORGE_LIVE_OLLAMA=1` because it requires a running Ollama
//! daemon and a pulled model. CI doesn't run it; local devs can.

use ollama_forge::providers::{GenerateOptions, LlmProvider, OllamaProvider};

fn live_model() -> Option<String> {
    std::env::var_os("FORGE_LIVE_OLLAMA")?;
    Some(std::env::var("FORGE_LIVE_MODEL").unwrap_or_else(|_| "llama3.2:latest".to_string()))
}

#[tokio::test]
async fn schema_constrained_returns_parseable_json() {
    let Some(model) = live_model() else {
        eprintln!("skipped: set FORGE_LIVE_OLLAMA=1 to run against a real ollama");
        return;
    };

    let ollama = OllamaProvider::new("http://localhost:11434");
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "age":  { "type": "integer" }
        },
        "required": ["name", "age"]
    });

    let opts = GenerateOptions {
        model,
        prompt: "Return a JSON object describing a fictional 30-year-old named Alice.".to_string(),
        system: Some(
            "You output only the requested JSON object, no markdown, no explanation.".to_string(),
        ),
        format: Some(schema),
        temperature: Some(0.1),
        stream: false,
        ..Default::default()
    };

    let resp = ollama.generate(opts).await.expect("ollama call");
    let parsed: serde_json::Value = serde_json::from_str(resp.content.trim())
        .unwrap_or_else(|e| panic!("model returned non-JSON: {e}\n---\n{}", resp.content));

    assert!(parsed.get("name").and_then(|v| v.as_str()).is_some());
    assert!(parsed.get("age").and_then(|v| v.as_i64()).is_some());
}
