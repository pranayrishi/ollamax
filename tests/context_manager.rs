//! Tests for the sliding-window context manager.
//!
//! Pin the contract: adding entries past `max_tokens` causes oldest entries
//! to fall out, total never exceeds the budget, and `clear()` actually
//! clears. Also pin the truncation walk used at prompt-build time.

use ollama_forge::context::ContextManager;

#[tokio::test]
async fn add_then_get_returns_content() {
    let cm = ContextManager::new(1000);
    cm.add("user", "hello world").await.unwrap();
    let ctx = cm.get_context(None).await.unwrap();
    assert!(ctx.contains("hello world"));
    assert!(ctx.contains("[user]"));
}

#[tokio::test]
async fn system_prompt_appears_first() {
    let cm = ContextManager::new(1000);
    cm.add("user", "second").await.unwrap();
    let ctx = cm.get_context(Some("YOU ARE A FROG")).await.unwrap();
    let frog = ctx.find("YOU ARE A FROG").unwrap();
    let second = ctx.find("second").unwrap();
    assert!(
        frog < second,
        "system prompt must come first in the context"
    );
}

#[tokio::test]
async fn sliding_window_evicts_old_entries() {
    // 3-token budget — every entry is ~2 tokens via the whitespace counter,
    // so the second add should evict the first.
    let cm = ContextManager::new(3);
    cm.add("user", "alpha beta").await.unwrap();
    cm.add("user", "gamma delta").await.unwrap();
    let stats = cm.stats().await;
    assert!(
        stats.total_tokens <= 3 || stats.entry_count == 1,
        "budget violated: {stats:?}"
    );
}

#[tokio::test]
async fn clear_resets_state() {
    let cm = ContextManager::new(1000);
    cm.add("user", "x y z").await.unwrap();
    cm.clear().await;
    let stats = cm.stats().await;
    assert_eq!(stats.entry_count, 0);
    assert_eq!(stats.total_tokens, 0);
}

#[tokio::test]
async fn get_truncated_context_respects_explicit_limit() {
    let cm = ContextManager::new(10_000);
    for i in 0..50 {
        cm.add("user", &format!("entry number {i}")).await.unwrap();
    }
    let truncated = cm.get_truncated_context(Some(20)).await.unwrap();
    assert!(truncated.total_tokens <= 20);
    assert!(truncated.truncated_count > 0);
}
