//! Configured, loopback-only OpenAI-compatible local endpoints.
//!
//! Ollama remains the default local runtime.  This registry is deliberately
//! opt-in for separately operated local servers such as vLLM, SGLang, or
//! llama.cpp's OpenAI-compatible server.  A catalog entry alone never starts
//! a server or makes a network connection: an operator must configure a
//! loopback endpoint and explicitly select `local:<endpoint>/<model>`.
//!
//! The registry performs three jobs:
//!
//! 1. validates selector syntax and exposes the declared model capabilities;
//! 2. creates an [`OpenAiCompatibleProvider`] only when that selector is used;
//! 3. wraps every provider call in a semaphore shared by all models at the
//!    same endpoint, so concurrent agent roles cannot accidentally overload a
//!    self-hosted inference server.

use super::{
    normalize_openai_compatible_endpoint, ChatOptions, GenerateOptions, LlmProvider, LlmResponse,
    ModelInfo, OpenAiCompatibleProvider,
};
use crate::{Config, LocalEndpointConfig, LocalEndpointModelConfig};
use anyhow::{Context, Result};
use serde::Serialize;
use std::{collections::BTreeMap, fmt, sync::Arc};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Prefix reserved for explicitly configured self-hosted local models.
pub const LOCAL_MODEL_SELECTOR_PREFIX: &str = "local:";

/// Parsed form of an explicit configured-local model selector.
///
/// This type intentionally contains only validated identifier segments. It
/// does not contain an endpoint URL, credentials, or a model server name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LocalModelSelector {
    pub endpoint_id: String,
    pub model_id: String,
}

impl LocalModelSelector {
    /// Canonical user-facing form: `local:<endpoint-id>/<model-id>`.
    pub fn as_selector(&self) -> String {
        format!(
            "{LOCAL_MODEL_SELECTOR_PREFIX}{}/{}",
            self.endpoint_id, self.model_id
        )
    }
}

impl fmt::Display for LocalModelSelector {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.as_selector())
    }
}

/// Parse a configured-local selector.
///
/// A non-local model name returns `Ok(None)`, letting normal Ollama routing
/// remain untouched. Values starting with `local:` are either parsed strictly
/// or rejected; accepting a malformed local selector as an ordinary model
/// would make a typo silently choose a different runtime.
pub fn parse_local_model_selector(value: &str) -> Result<Option<LocalModelSelector>> {
    if value.trim() != value && value.trim().starts_with(LOCAL_MODEL_SELECTOR_PREFIX) {
        anyhow::bail!(
            "configured local model selector must not have surrounding whitespace; expected `local:<endpoint-id>/<model-id>`"
        );
    }
    if !value.starts_with(LOCAL_MODEL_SELECTOR_PREFIX) {
        return Ok(None);
    }

    let remainder = &value[LOCAL_MODEL_SELECTOR_PREFIX.len()..];
    let Some((endpoint_id, model_id)) = remainder.split_once('/') else {
        anyhow::bail!(
            "invalid configured local model selector `{value}`; expected `local:<endpoint-id>/<model-id>`"
        );
    };
    if model_id.contains('/') {
        anyhow::bail!(
            "invalid configured local model selector `{value}`; it must contain exactly one `/` after `local:`"
        );
    }
    if !is_safe_selector_segment(endpoint_id) || !is_safe_selector_segment(model_id) {
        anyhow::bail!(
            "invalid configured local model selector `{value}`; endpoint and model ids may contain only letters, digits, `.`, `_`, or `-`"
        );
    }

    Ok(Some(LocalModelSelector {
        endpoint_id: endpoint_id.to_string(),
        model_id: model_id.to_string(),
    }))
}

/// Serialisable, credential-free disclosure for one configured local model.
///
/// `endpoint_url` is already constrained to a normalized loopback `/v1` URL;
/// the API-key environment variable is deliberately never included here.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConfiguredLocalModelMetadata {
    pub selector: String,
    pub endpoint_id: String,
    pub model_id: String,
    /// Exact model string sent to the separately operated local server.
    pub served_model: String,
    pub label: Option<String>,
    pub endpoint_url: String,
    pub max_parallel_requests: usize,
    pub vision: bool,
    pub thinking: bool,
    pub context_window_tokens: Option<usize>,
}

/// A resolved configured local model and its bounded provider.
///
/// The provider is created lazily by [`LocalEndpointRegistry::resolve`].
/// It always sends `served_model` to the endpoint even if a caller retains
/// the public selector in its local configuration.
#[derive(Clone)]
pub struct ConfiguredLocalModel {
    pub selector: String,
    pub endpoint_id: String,
    pub model_id: String,
    pub served_model: String,
    pub label: Option<String>,
    pub endpoint_url: String,
    pub max_parallel_requests: usize,
    pub vision: bool,
    pub thinking: bool,
    pub context_window_tokens: Option<usize>,
    pub provider: Arc<dyn LlmProvider>,
}

impl fmt::Debug for ConfiguredLocalModel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConfiguredLocalModel")
            .field("selector", &self.selector)
            .field("endpoint_id", &self.endpoint_id)
            .field("model_id", &self.model_id)
            .field("served_model", &self.served_model)
            .field("label", &self.label)
            .field("endpoint_url", &self.endpoint_url)
            .field("max_parallel_requests", &self.max_parallel_requests)
            .field("vision", &self.vision)
            .field("thinking", &self.thinking)
            .field("context_window_tokens", &self.context_window_tokens)
            .field("provider", &self.provider.name())
            .finish()
    }
}

impl ConfiguredLocalModel {
    pub fn metadata(&self) -> ConfiguredLocalModelMetadata {
        ConfiguredLocalModelMetadata {
            selector: self.selector.clone(),
            endpoint_id: self.endpoint_id.clone(),
            model_id: self.model_id.clone(),
            served_model: self.served_model.clone(),
            label: self.label.clone(),
            endpoint_url: self.endpoint_url.clone(),
            max_parallel_requests: self.max_parallel_requests,
            vision: self.vision,
            thinking: self.thinking,
            context_window_tokens: self.context_window_tokens,
        }
    }
}

#[derive(Clone)]
pub struct LocalEndpointRegistry {
    endpoints: Arc<BTreeMap<String, EndpointEntry>>,
}

impl fmt::Debug for LocalEndpointRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalEndpointRegistry")
            .field("configured_models", &self.configured_models())
            .finish()
    }
}

#[derive(Clone)]
struct EndpointEntry {
    id: String,
    url: String,
    api_key_env: Option<String>,
    max_parallel_requests: usize,
    models: BTreeMap<String, EndpointModelEntry>,
    permits: Arc<Semaphore>,
}

#[derive(Clone)]
struct EndpointModelEntry {
    id: String,
    served_model: String,
    label: Option<String>,
    vision: bool,
    thinking: bool,
    context_window_tokens: Option<usize>,
}

impl LocalEndpointRegistry {
    /// Build an inert registry from already-loaded local endpoint config.
    ///
    /// This performs syntax and loopback validation only. It does *not* read
    /// `api_key_env`, build an HTTP client, connect to a server, or load a
    /// model. Those actions happen only in [`Self::resolve`].
    pub fn new(configs: &[LocalEndpointConfig]) -> Result<Self> {
        let mut endpoints = BTreeMap::new();
        let mut endpoint_urls = BTreeMap::new();
        for config in configs {
            let endpoint = EndpointEntry::from_config(config)?;
            if let Some(existing_id) =
                endpoint_urls.insert(endpoint.url.clone(), endpoint.id.clone())
            {
                anyhow::bail!(
                    "local endpoint `{}` duplicates normalized loopback URL already declared by `{existing_id}`; combine their models under one endpoint so its request limit remains enforceable",
                    endpoint.id
                );
            }
            if endpoints.insert(endpoint.id.clone(), endpoint).is_some() {
                anyhow::bail!("duplicate local endpoint id `{}`", config.id);
            }
        }
        Ok(Self {
            endpoints: Arc::new(endpoints),
        })
    }

    /// Convenience constructor for the application's complete configuration.
    pub fn from_config(config: &Config) -> Result<Self> {
        Self::new(&config.local_endpoints)
    }

    /// Return all configured local models in deterministic selector order.
    ///
    /// This is safe to expose in a picker or status API: it contains neither
    /// secrets nor unverified hardware-fit claims.
    pub fn configured_models(&self) -> Vec<ConfiguredLocalModelMetadata> {
        self.endpoints
            .values()
            .flat_map(|endpoint| {
                endpoint
                    .models
                    .values()
                    .map(move |model| endpoint.metadata_for(model))
            })
            .collect()
    }

    /// Look up metadata without constructing a provider.
    pub fn metadata(&self, selector: &str) -> Result<ConfiguredLocalModelMetadata> {
        let selector = parse_local_model_selector(selector)?.ok_or_else(|| {
            anyhow::anyhow!(
                "`{selector}` is not a configured local selector; expected `local:<endpoint-id>/<model-id>`"
            )
        })?;
        let (endpoint, model) = self.lookup(&selector)?;
        Ok(endpoint.metadata_for(model))
    }

    /// Resolve an explicitly configured local selector into a bounded provider.
    ///
    /// Missing bearer-token environment variables are intentionally reported
    /// here, when a user chose this endpoint, rather than preventing the app
    /// from starting with unrelated local models available.
    pub fn resolve(&self, selector: &str) -> Result<ConfiguredLocalModel> {
        let selector = parse_local_model_selector(selector)?.ok_or_else(|| {
            anyhow::anyhow!(
                "`{selector}` is not a configured local selector; expected `local:<endpoint-id>/<model-id>`"
            )
        })?;
        self.resolve_selector(&selector)
    }

    /// Resolve only if `selected_model` uses the `local:` namespace.
    ///
    /// Normal Ollama model names return `Ok(None)`, keeping Auto and existing
    /// explicit Ollama paths separate from this opt-in endpoint mechanism.
    pub fn try_resolve(&self, selected_model: &str) -> Result<Option<ConfiguredLocalModel>> {
        let Some(selector) = parse_local_model_selector(selected_model)? else {
            return Ok(None);
        };
        self.resolve_selector(&selector).map(Some)
    }

    pub fn is_empty(&self) -> bool {
        self.endpoints.is_empty()
    }

    fn resolve_selector(&self, selector: &LocalModelSelector) -> Result<ConfiguredLocalModel> {
        let (endpoint, model) = self.lookup(selector)?;
        // Construct this only for an actual user selection. The provider
        // re-normalizes the URL defensively and reads the named env var here.
        let provider = OpenAiCompatibleProvider::try_new_with_api_key_env(
            &endpoint.url,
            endpoint.api_key_env.as_deref(),
        )
        .with_context(|| {
            format!(
                "initialize configured local endpoint `{}` for selector `{}`",
                endpoint.id, selector
            )
        })?;
        let metadata = endpoint.metadata_for(model);
        let provider_name = format!("openai-compatible-local:{}", endpoint.id);
        let bounded: Arc<dyn LlmProvider> = Arc::new(BoundedLlmProvider::new(
            Arc::new(provider),
            endpoint.permits.clone(),
            model.served_model.clone(),
            provider_name,
        ));

        Ok(ConfiguredLocalModel {
            selector: metadata.selector,
            endpoint_id: metadata.endpoint_id,
            model_id: metadata.model_id,
            served_model: metadata.served_model,
            label: metadata.label,
            endpoint_url: metadata.endpoint_url,
            max_parallel_requests: metadata.max_parallel_requests,
            vision: metadata.vision,
            thinking: metadata.thinking,
            context_window_tokens: metadata.context_window_tokens,
            provider: bounded,
        })
    }

    fn lookup(
        &self,
        selector: &LocalModelSelector,
    ) -> Result<(&EndpointEntry, &EndpointModelEntry)> {
        let endpoint = self.endpoints.get(&selector.endpoint_id).ok_or_else(|| {
            anyhow::anyhow!(
                "configured local endpoint `{}` was not found for selector `{}`",
                selector.endpoint_id,
                selector
            )
        })?;
        let model = endpoint.models.get(&selector.model_id).ok_or_else(|| {
            anyhow::anyhow!(
                "configured local model `{}/{}` was not found; declare it under `[[local_endpoints.models]]`",
                selector.endpoint_id,
                selector.model_id
            )
        })?;
        Ok((endpoint, model))
    }
}

impl Default for LocalEndpointRegistry {
    fn default() -> Self {
        Self {
            endpoints: Arc::new(BTreeMap::new()),
        }
    }
}

impl EndpointEntry {
    fn from_config(config: &LocalEndpointConfig) -> Result<Self> {
        if !is_safe_selector_segment(&config.id) {
            anyhow::bail!(
                "local endpoint id `{}` must use only letters, digits, `.`, `_`, or `-`",
                config.id
            );
        }
        let url = normalize_openai_compatible_endpoint(&config.url)
            .with_context(|| format!("normalize configured local endpoint `{}`", config.id))?;
        if config.models.is_empty() {
            anyhow::bail!(
                "local endpoint `{}` must declare at least one served model",
                config.id
            );
        }

        let max_parallel_requests = config.max_parallel_requests.clamp(1, 16);
        let mut models = BTreeMap::new();
        for model in &config.models {
            let entry = EndpointModelEntry::from_config(&config.id, model)?;
            if models.insert(entry.id.clone(), entry).is_some() {
                anyhow::bail!(
                    "duplicate local endpoint model id `{}/{}`",
                    config.id,
                    model.id
                );
            }
        }

        Ok(Self {
            id: config.id.clone(),
            url,
            api_key_env: config.api_key_env.clone(),
            max_parallel_requests,
            models,
            permits: Arc::new(Semaphore::new(max_parallel_requests)),
        })
    }

    fn metadata_for(&self, model: &EndpointModelEntry) -> ConfiguredLocalModelMetadata {
        ConfiguredLocalModelMetadata {
            selector: format!("{LOCAL_MODEL_SELECTOR_PREFIX}{}/{}", self.id, model.id),
            endpoint_id: self.id.clone(),
            model_id: model.id.clone(),
            served_model: model.served_model.clone(),
            label: model.label.clone(),
            endpoint_url: self.url.clone(),
            max_parallel_requests: self.max_parallel_requests,
            vision: model.vision,
            thinking: model.thinking,
            context_window_tokens: model.context_window_tokens,
        }
    }
}

impl EndpointModelEntry {
    fn from_config(endpoint_id: &str, config: &LocalEndpointModelConfig) -> Result<Self> {
        if !is_safe_selector_segment(&config.id) {
            anyhow::bail!(
                "local endpoint model id `{endpoint_id}/{}` must use only letters, digits, `.`, `_`, or `-`",
                config.id
            );
        }
        if config.served_model.trim().is_empty() {
            anyhow::bail!(
                "local endpoint model `{endpoint_id}/{}` has an empty served_model",
                config.id
            );
        }
        Ok(Self {
            id: config.id.clone(),
            served_model: config.served_model.trim().to_string(),
            label: config
                .label
                .clone()
                .filter(|label| !label.trim().is_empty()),
            vision: config.vision,
            thinking: config.thinking,
            context_window_tokens: config.context_window_tokens,
        })
    }
}

fn is_safe_selector_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// Provider wrapper that gives all models served by an endpoint one bounded
/// request lane. It also translates the public `local:<endpoint>/<model>`
/// selector into the server's exact `served_model` string.
#[derive(Clone)]
struct BoundedLlmProvider {
    inner: Arc<dyn LlmProvider>,
    permits: Arc<Semaphore>,
    served_model: String,
    provider_name: String,
}

impl BoundedLlmProvider {
    fn new(
        inner: Arc<dyn LlmProvider>,
        permits: Arc<Semaphore>,
        served_model: String,
        provider_name: String,
    ) -> Self {
        Self {
            inner,
            permits,
            served_model,
            provider_name,
        }
    }

    async fn acquire(&self) -> Result<OwnedSemaphorePermit> {
        self.permits.clone().acquire_owned().await.map_err(|_| {
            anyhow::anyhow!(
                "configured local endpoint request limiter was closed before a request could start"
            )
        })
    }
}

#[async_trait::async_trait]
impl LlmProvider for BoundedLlmProvider {
    fn name(&self) -> &str {
        &self.provider_name
    }

    async fn generate(&self, mut options: GenerateOptions) -> Result<LlmResponse> {
        let _permit = self.acquire().await?;
        options.model.clone_from(&self.served_model);
        self.inner.generate(options).await
    }

    async fn chat(&self, mut options: ChatOptions) -> Result<LlmResponse> {
        let _permit = self.acquire().await?;
        options.model.clone_from(&self.served_model);
        self.inner.chat(options).await
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        let _permit = self.acquire().await?;
        self.inner.list_models().await
    }

    async fn model_fingerprint(&self, _model: &str) -> Option<String> {
        let _permit = self.permits.clone().acquire_owned().await.ok()?;
        self.inner.model_fingerprint(&self.served_model).await
    }

    async fn preload(&self, _model: &str, keep_alive: &str) -> Result<()> {
        let _permit = self.acquire().await?;
        self.inner.preload(&self.served_model, keep_alive).await
    }
}

#[cfg(test)]
mod tests {
    use super::{
        parse_local_model_selector, BoundedLlmProvider, LocalEndpointRegistry,
        LOCAL_MODEL_SELECTOR_PREFIX,
    };
    use crate::{
        providers::{ChatOptions, GenerateOptions, LlmProvider, LlmResponse, ModelInfo},
        LocalEndpointConfig, LocalEndpointModelConfig,
    };
    use anyhow::Result;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use tokio::{
        sync::Semaphore,
        time::{sleep, Duration},
    };

    fn endpoint(
        id: &str,
        url: &str,
        max_parallel_requests: usize,
        models: Vec<LocalEndpointModelConfig>,
    ) -> LocalEndpointConfig {
        LocalEndpointConfig {
            id: id.to_string(),
            url: url.to_string(),
            api_key_env: None,
            max_parallel_requests,
            models,
        }
    }

    fn model(id: &str, served_model: &str) -> LocalEndpointModelConfig {
        LocalEndpointModelConfig {
            id: id.to_string(),
            served_model: served_model.to_string(),
            label: None,
            vision: false,
            thinking: false,
            context_window_tokens: None,
        }
    }

    #[test]
    fn selectors_are_strict_and_non_local_names_stay_unclaimed() {
        let parsed = parse_local_model_selector("local:deepseek-v4/flash")
            .unwrap()
            .unwrap();
        assert_eq!(parsed.endpoint_id, "deepseek-v4");
        assert_eq!(parsed.model_id, "flash");
        assert_eq!(parsed.to_string(), "local:deepseek-v4/flash");
        assert_eq!(LOCAL_MODEL_SELECTOR_PREFIX, "local:");
        assert!(parse_local_model_selector("qwen3.5:4b").unwrap().is_none());

        for invalid in [
            "local:",
            "local:deepseek-v4",
            "local:deepseek-v4/flash/extra",
            "local:deepseek v4/flash",
            " local:deepseek-v4/flash",
        ] {
            assert!(
                parse_local_model_selector(invalid).is_err(),
                "should reject {invalid}"
            );
        }
    }

    #[test]
    fn registry_exposes_only_declared_capabilities_and_validates_loopback() {
        let registry = LocalEndpointRegistry::new(&[endpoint(
            "lab",
            "http://localhost:8000",
            99,
            vec![LocalEndpointModelConfig {
                id: "vision".to_string(),
                served_model: "Gemma-4-12B".to_string(),
                label: Some("Lab visual model".to_string()),
                vision: true,
                thinking: true,
                context_window_tokens: Some(65_536),
            }],
        )])
        .unwrap();
        assert_eq!(registry.configured_models().len(), 1);
        let metadata = registry.metadata("local:lab/vision").unwrap();
        assert_eq!(metadata.endpoint_url, "http://127.0.0.1:8000/v1");
        assert_eq!(metadata.max_parallel_requests, 16);
        assert_eq!(metadata.served_model, "Gemma-4-12B");
        assert_eq!(metadata.label.as_deref(), Some("Lab visual model"));
        assert!(metadata.vision);
        assert!(metadata.thinking);
        assert_eq!(metadata.context_window_tokens, Some(65_536));

        let remote = LocalEndpointRegistry::new(&[endpoint(
            "not-local",
            "https://example.com/v1",
            1,
            vec![model("m", "remote")],
        )])
        .unwrap_err();
        assert!(format!("{remote:#}").contains("loopback"));
    }

    #[test]
    fn registry_rejects_aliases_for_one_normalized_local_server() {
        let error = LocalEndpointRegistry::new(&[
            endpoint("first", "http://localhost:8000", 1, vec![model("a", "A")]),
            endpoint(
                "second",
                "http://127.0.0.1:8000/v1",
                8,
                vec![model("b", "B")],
            ),
        ])
        .unwrap_err();
        let rendered = format!("{error:#}");
        assert!(rendered.contains("duplicates normalized loopback URL"));
        assert!(rendered.contains("combine their models"));
    }

    #[test]
    fn provider_and_api_key_are_lazy_until_a_configured_model_is_selected() {
        let missing_env = format!(
            "OLLAMAX_MISSING_LOCAL_TEST_{}",
            uuid::Uuid::new_v4().simple()
        );
        let registry = LocalEndpointRegistry::new(&[LocalEndpointConfig {
            id: "private-lab".to_string(),
            url: "http://127.0.0.1:9999".to_string(),
            api_key_env: Some(missing_env.clone()),
            max_parallel_requests: 1,
            models: vec![model("m3", "MiniMax-M3")],
        }])
        .unwrap();

        // Startup/picker paths do not read the missing secret.
        assert_eq!(
            registry.configured_models()[0].selector,
            "local:private-lab/m3"
        );
        let error = registry.resolve("local:private-lab/m3").unwrap_err();
        let rendered = format!("{error:#}");
        assert!(rendered.contains(&missing_env));
        assert!(!rendered.contains("MiniMax-M3"));
    }

    struct CountingProvider {
        active: AtomicUsize,
        peak: AtomicUsize,
        observed_models: std::sync::Mutex<Vec<String>>,
    }

    impl CountingProvider {
        fn note_active(&self) {
            let current = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(current, Ordering::SeqCst);
        }

        fn finish(&self) {
            self.active.fetch_sub(1, Ordering::SeqCst);
        }
    }

    #[async_trait::async_trait]
    impl LlmProvider for CountingProvider {
        fn name(&self) -> &str {
            "counting-test-provider"
        }

        async fn generate(&self, options: GenerateOptions) -> Result<LlmResponse> {
            self.observed_models
                .lock()
                .unwrap()
                .push(options.model.clone());
            self.note_active();
            sleep(Duration::from_millis(25)).await;
            self.finish();
            Ok(LlmResponse {
                content: "ok".to_string(),
                model: options.model,
                tokens_generated: 1,
                context_used: 1,
                duration_ms: 25,
            })
        }

        async fn chat(&self, _options: ChatOptions) -> Result<LlmResponse> {
            unreachable!("this test exercises generate")
        }

        async fn list_models(&self) -> Result<Vec<ModelInfo>> {
            Ok(Vec::new())
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shared_limiter_bounds_calls_and_rewrites_to_served_model() {
        let inner = Arc::new(CountingProvider {
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            observed_models: std::sync::Mutex::new(Vec::new()),
        });
        let limiter = Arc::new(Semaphore::new(2));
        let first = BoundedLlmProvider::new(
            inner.clone(),
            limiter.clone(),
            "served-A".to_string(),
            "test-endpoint".to_string(),
        );
        let second = BoundedLlmProvider::new(
            inner.clone(),
            limiter,
            "served-B".to_string(),
            "test-endpoint".to_string(),
        );

        let mut tasks = Vec::new();
        for index in 0..8 {
            let provider = if index % 2 == 0 {
                first.clone()
            } else {
                second.clone()
            };
            tasks.push(tokio::spawn(async move {
                provider
                    .generate(GenerateOptions {
                        model: "local:lab/ignored".to_string(),
                        ..Default::default()
                    })
                    .await
                    .unwrap();
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }

        assert_eq!(inner.peak.load(Ordering::SeqCst), 2);
        let seen = inner.observed_models.lock().unwrap().clone();
        assert_eq!(seen.len(), 8);
        assert!(seen
            .iter()
            .all(|model| model == "served-A" || model == "served-B"));
        assert!(!seen.iter().any(|model| model.starts_with("local:")));
    }
}
