//! Model provider abstraction layer for APEX.

pub mod cost;
pub mod fake;
pub mod openai;
pub mod provider;

use std::sync::Arc;

use apex_core::config::{Config, ProviderKind};
use apex_core::error::{ApexError, Result};

pub use fake::FakeProvider;
pub use openai::OpenAiCompatibleProvider;
pub use provider::{
    CompletionRequest, CompletionResponse, ModelCapabilities, ModelProvider, StreamChunk,
};

/// Build a provider adapter from configuration.
pub fn build_provider(config: &Config, provider_name: &str) -> Result<Arc<dyn ModelProvider>> {
    let provider = config.providers.get(provider_name).ok_or_else(|| {
        ApexError::config(format!("provider '{provider_name}' is not configured"))
    })?;

    match provider.kind {
        ProviderKind::Fake => Ok(Arc::new(FakeProvider::with_default_text(
            "Fake provider: configure a real provider with `apex models set`.",
        ))),
        ProviderKind::OpenAiCompatible => {
            let api_key = provider
                .api_key_env
                .as_ref()
                .and_then(|var| std::env::var(var).ok())
                .filter(|k| !k.is_empty());
            let adapter =
                OpenAiCompatibleProvider::new(provider_name, provider.base_url.clone(), api_key)?;
            Ok(Arc::new(adapter))
        }
    }
}

/// Build a provider for a `provider/model` reference, returning the adapter and
/// the bare model id.
pub fn build_for_reference(
    config: &Config,
    reference: &str,
) -> Result<(Arc<dyn ModelProvider>, String)> {
    let (provider, model) = config.resolve_model(Some(reference))?;
    Ok((build_provider(config, &provider)?, model))
}
