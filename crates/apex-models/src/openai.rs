//! OpenAI-compatible Chat Completions adapter.
//!
//! Works against the OpenAI API and any endpoint that implements the same
//! `/chat/completions` contract (including many local model servers).

use std::time::Duration;

use apex_core::error::{ApexError, Result};
use apex_protocol::{ChatMessage, Role, ToolCall, ToolSpec, Usage};
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::cost::estimate_cost;
use crate::provider::{CompletionRequest, CompletionResponse, ModelCapabilities, ModelProvider};

const MAX_RETRIES: u32 = 3;

/// A provider that speaks the OpenAI Chat Completions API.
pub struct OpenAiCompatibleProvider {
    name: String,
    base_url: String,
    api_key: Option<String>,
    client: reqwest::Client,
}

impl OpenAiCompatibleProvider {
    /// Build a provider. `base_url` should include the API root (for example
    /// `https://api.openai.com/v1`).
    pub fn new(
        name: impl Into<String>,
        base_url: impl Into<String>,
        api_key: Option<String>,
    ) -> Result<Self> {
        let name = name.into();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| ApexError::model(&name, format!("failed to build HTTP client: {e}")))?;
        Ok(OpenAiCompatibleProvider {
            name,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key,
            client,
        })
    }

    fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }

    fn build_body(&self, request: &CompletionRequest) -> Value {
        let messages: Vec<Value> = request.messages.iter().map(encode_message).collect();
        let mut body = json!({
            "model": request.model,
            "messages": messages,
        });
        if let Some(temp) = request.temperature {
            body["temperature"] = json!(temp);
        }
        if let Some(max) = request.max_tokens {
            body["max_tokens"] = json!(max);
        }
        if !request.tools.is_empty() {
            let tools: Vec<Value> = request.tools.iter().map(encode_tool).collect();
            body["tools"] = json!(tools);
            body["tool_choice"] = json!("auto");
        }
        body
    }

    async fn post(&self, body: &Value) -> Result<reqwest::Response> {
        let mut request = self.client.post(self.endpoint()).json(body);
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key);
        }
        request
            .send()
            .await
            .map_err(|e| ApexError::model(&self.name, format!("request failed: {e}")))
    }
}

#[async_trait]
impl ModelProvider for OpenAiCompatibleProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn capabilities(&self, model: &str) -> ModelCapabilities {
        ModelCapabilities {
            supports_tools: true,
            supports_streaming: true,
            context_window: context_window_for(model),
            max_output_tokens: 4096,
        }
    }

    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse> {
        let body = self.build_body(&request);
        let mut attempt = 0;
        loop {
            attempt += 1;
            let response = self.post(&body).await?;
            let status = response.status();

            if status.is_success() {
                let text = response.text().await.map_err(|e| {
                    ApexError::model(&self.name, format!("failed to read response body: {e}"))
                })?;
                return parse_response(&self.name, &request.model, &text);
            }

            let retryable = status.as_u16() == 429 || status.is_server_error();
            let body_text = response.text().await.unwrap_or_default();
            if retryable && attempt < MAX_RETRIES {
                let backoff = Duration::from_millis(300 * 2u64.pow(attempt - 1));
                tracing::warn!(
                    provider = %self.name,
                    attempt,
                    status = status.as_u16(),
                    "retrying model request after transient error"
                );
                tokio::time::sleep(backoff).await;
                continue;
            }

            return Err(ApexError::model(
                &self.name,
                format!(
                    "HTTP {} from {}: {}",
                    status.as_u16(),
                    self.endpoint(),
                    truncate(&body_text, 500)
                ),
            ));
        }
    }
}

fn encode_message(message: &ChatMessage) -> Value {
    match message.role {
        Role::Assistant if !message.tool_calls.is_empty() => {
            let calls: Vec<Value> = message
                .tool_calls
                .iter()
                .map(|call| {
                    json!({
                        "id": call.id,
                        "type": "function",
                        "function": {
                            "name": call.name,
                            "arguments": call.arguments.to_string(),
                        }
                    })
                })
                .collect();
            json!({
                "role": "assistant",
                "content": message.content,
                "tool_calls": calls,
            })
        }
        Role::Tool => json!({
            "role": "tool",
            "tool_call_id": message.tool_call_id,
            "content": message.content.clone().unwrap_or_default(),
        }),
        Role::System => json!({"role": "system", "content": message.content}),
        Role::User => json!({"role": "user", "content": message.content}),
        Role::Assistant => json!({"role": "assistant", "content": message.content}),
    }
}

fn encode_tool(spec: &ToolSpec) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": spec.name,
            "description": spec.description,
            "parameters": spec.parameters,
        }
    })
}

fn parse_response(provider: &str, model: &str, text: &str) -> Result<CompletionResponse> {
    let value: Value = serde_json::from_str(text)
        .map_err(|e| ApexError::model(provider, format!("invalid JSON response: {e}")))?;

    let choice = value
        .get("choices")
        .and_then(|c| c.get(0))
        .ok_or_else(|| ApexError::model(provider, "response contained no choices"))?;

    let message = choice
        .get("message")
        .ok_or_else(|| ApexError::model(provider, "response choice contained no message"))?;

    let content = message
        .get("content")
        .and_then(|c| c.as_str())
        .map(|s| s.to_string());

    let mut tool_calls = Vec::new();
    if let Some(calls) = message.get("tool_calls").and_then(|c| c.as_array()) {
        for call in calls {
            let id = call
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let function = call.get("function").cloned().unwrap_or(Value::Null);
            let name = function
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let arguments = match function.get("arguments") {
                Some(Value::String(s)) => {
                    serde_json::from_str(s).unwrap_or_else(|_| json!({ "raw": s }))
                }
                Some(other) => other.clone(),
                None => json!({}),
            };
            tool_calls.push(ToolCall {
                id,
                name,
                arguments,
            });
        }
    }

    let finish_reason = choice
        .get("finish_reason")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let mut usage = Usage::default();
    if let Some(u) = value.get("usage") {
        usage.prompt_tokens = u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        usage.completion_tokens = u
            .get("completion_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        usage.total_tokens = u.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
    }
    if usage.total_tokens == 0 {
        usage.total_tokens = usage.prompt_tokens + usage.completion_tokens;
    }
    usage.cost_usd = estimate_cost(model, &usage);

    Ok(CompletionResponse {
        message: ChatMessage {
            role: Role::Assistant,
            content,
            tool_calls,
            tool_call_id: None,
            name: None,
        },
        usage,
        finish_reason,
    })
}

fn context_window_for(model: &str) -> u32 {
    if model.contains("gpt-4") || model.contains("o3") || model.contains("o4") {
        128_000
    } else if model.contains("gpt-3.5") {
        16_385
    } else {
        32_768
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        text.to_string()
    } else {
        format!("{}…", &text[..max])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_tool_calls() {
        let message = ChatMessage {
            role: Role::Assistant,
            content: None,
            tool_calls: vec![ToolCall {
                id: "call_1".into(),
                name: "read_file".into(),
                arguments: json!({"path": "a.rs"}),
            }],
            tool_call_id: None,
            name: None,
        };
        let encoded = encode_message(&message);
        assert_eq!(encoded["tool_calls"][0]["function"]["name"], "read_file");
    }

    #[test]
    fn parses_tool_call_response() {
        let text = r#"{
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {"name": "read_file", "arguments": "{\"path\": \"x\"}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        }"#;
        let response = parse_response("test", "gpt-4o-mini", text).unwrap();
        assert_eq!(response.message.tool_calls.len(), 1);
        assert_eq!(response.message.tool_calls[0].name, "read_file");
        assert_eq!(response.message.tool_calls[0].arguments["path"], "x");
        assert_eq!(response.usage.total_tokens, 15);
    }
}
