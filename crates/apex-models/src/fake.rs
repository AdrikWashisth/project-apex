//! A deterministic, offline provider used by tests and local demos.
//!
//! It never performs network I/O. Scripted responses are returned in order;
//! once the script is exhausted a configured default is returned.

use std::collections::VecDeque;
use std::sync::Mutex;

use apex_core::error::Result;
use apex_protocol::{ChatMessage, Role, ToolCall, Usage};
use async_trait::async_trait;
use serde_json::Value;

use crate::provider::{CompletionRequest, CompletionResponse, ModelCapabilities, ModelProvider};

/// A scripted provider.
pub struct FakeProvider {
    name: String,
    responses: Mutex<VecDeque<CompletionResponse>>,
    default: CompletionResponse,
}

impl FakeProvider {
    /// Create a provider that always returns a simple completion.
    pub fn new() -> Self {
        Self::with_default_text("Fake provider complete.")
    }

    /// Create a provider with a fixed default reply.
    pub fn with_default_text(text: impl Into<String>) -> Self {
        FakeProvider {
            name: "fake".into(),
            responses: Mutex::new(VecDeque::new()),
            default: CompletionResponse::text(text),
        }
    }

    /// Queue a response to be returned on the next call.
    pub fn push(&self, response: CompletionResponse) {
        self.responses.lock().unwrap().push_back(response);
    }

    /// Replace the script with the given responses.
    pub fn set_script(&self, responses: Vec<CompletionResponse>) {
        let mut queue = self.responses.lock().unwrap();
        queue.clear();
        queue.extend(responses);
    }

    /// Build an assistant response that requests one tool call.
    pub fn tool_call(
        call_id: impl Into<String>,
        name: impl Into<String>,
        arguments: Value,
    ) -> CompletionResponse {
        let mut message = ChatMessage::assistant("");
        message.content = None;
        message.role = Role::Assistant;
        message.tool_calls = vec![ToolCall {
            id: call_id.into(),
            name: name.into(),
            arguments,
        }];
        CompletionResponse {
            message,
            usage: Usage {
                prompt_tokens: 5,
                completion_tokens: 5,
                total_tokens: 10,
                cost_usd: Some(0.0),
            },
            finish_reason: Some("tool_calls".into()),
        }
    }

    /// Build a final assistant text response.
    pub fn text_reply(text: impl Into<String>) -> CompletionResponse {
        CompletionResponse {
            message: ChatMessage::assistant(text),
            usage: Usage {
                prompt_tokens: 5,
                completion_tokens: 5,
                total_tokens: 10,
                cost_usd: Some(0.0),
            },
            finish_reason: Some("stop".into()),
        }
    }
}

impl Default for FakeProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModelProvider for FakeProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn capabilities(&self, _model: &str) -> ModelCapabilities {
        ModelCapabilities {
            supports_tools: true,
            supports_streaming: false,
            context_window: 100_000,
            max_output_tokens: 4096,
        }
    }

    async fn complete(&self, _request: CompletionRequest) -> Result<CompletionResponse> {
        let mut queue = self.responses.lock().unwrap();
        if let Some(response) = queue.pop_front() {
            Ok(response)
        } else {
            Ok(self.default.clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::CompletionRequest;

    #[tokio::test]
    async fn returns_scripted_then_default() {
        let provider = FakeProvider::with_default_text("default reply");
        provider.set_script(vec![FakeProvider::text_reply("first")]);

        let first = provider
            .complete(CompletionRequest::new("fake", vec![]))
            .await
            .unwrap();
        assert_eq!(first.message.content.as_deref(), Some("first"));

        let second = provider
            .complete(CompletionRequest::new("fake", vec![]))
            .await
            .unwrap();
        assert_eq!(second.message.content.as_deref(), Some("default reply"));
    }
}
