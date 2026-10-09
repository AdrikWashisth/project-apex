//! The provider abstraction every model adapter implements.

use apex_core::error::Result;
use apex_protocol::{ChatMessage, ToolSpec, Usage};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Capabilities a specific model supports.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCapabilities {
    /// Whether the model accepts a tools array.
    pub supports_tools: bool,
    /// Whether the provider can stream tokens.
    pub supports_streaming: bool,
    /// Maximum context window in tokens.
    pub context_window: u32,
    /// Maximum output tokens.
    pub max_output_tokens: u32,
}

impl Default for ModelCapabilities {
    fn default() -> Self {
        ModelCapabilities {
            supports_tools: true,
            supports_streaming: false,
            context_window: 128_000,
            max_output_tokens: 4096,
        }
    }
}

/// A single completion request.
#[derive(Debug, Clone)]
pub struct CompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolSpec>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

impl CompletionRequest {
    pub fn new(model: impl Into<String>, messages: Vec<ChatMessage>) -> Self {
        CompletionRequest {
            model: model.into(),
            messages,
            tools: Vec::new(),
            temperature: None,
            max_tokens: None,
        }
    }

    pub fn with_tools(mut self, tools: Vec<ToolSpec>) -> Self {
        self.tools = tools;
        self
    }

    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }
}

/// A completion response.
#[derive(Debug, Clone)]
pub struct CompletionResponse {
    /// The assistant message (text and/or tool calls).
    pub message: ChatMessage,
    /// Token and cost accounting.
    pub usage: Usage,
    /// Provider finish reason, if reported.
    pub finish_reason: Option<String>,
}

impl CompletionResponse {
    /// Convenience constructor for a plain assistant text reply.
    pub fn text(content: impl Into<String>) -> Self {
        CompletionResponse {
            message: ChatMessage::assistant(content),
            usage: Usage::default(),
            finish_reason: Some("stop".into()),
        }
    }
}

/// A chunk emitted during streaming. Reserved for providers that support it.
#[derive(Debug, Clone)]
pub struct StreamChunk {
    /// Incremental text content.
    pub delta: String,
    /// Whether this is the final chunk.
    pub done: bool,
}

/// The common interface implemented by every model adapter.
#[async_trait]
pub trait ModelProvider: Send + Sync {
    /// Provider name as configured.
    fn name(&self) -> &str;

    /// Capabilities for the given model id.
    fn capabilities(&self, model: &str) -> ModelCapabilities;

    /// Perform a non-streaming completion.
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse>;
}
