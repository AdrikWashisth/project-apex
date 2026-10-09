//! Shared conversation and tool-call model used by providers, agents and the
//! persisted event log.

use serde::{Deserialize, Serialize};

/// Role of a chat message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// A model tool invocation request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    /// Provider-assigned call identifier.
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Parsed JSON arguments.
    pub arguments: serde_json::Value,
}

/// A single message in a conversation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatMessage {
    /// Author role.
    pub role: Role,
    /// Text content (may be `None` for tool-call-only assistant turns).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Tool calls requested by the assistant.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// For `role == Tool`, the originating call id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// For `role == Tool`, the tool name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        ChatMessage {
            role: Role::System,
            content: Some(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        ChatMessage {
            role: Role::User,
            content: Some(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        ChatMessage {
            role: Role::Assistant,
            content: Some(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
        }
    }

    pub fn tool_result(
        call_id: impl Into<String>,
        name: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        ChatMessage {
            role: Role::Tool,
            content: Some(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: Some(call_id.into()),
            name: Some(name.into()),
        }
    }
}

/// Declares a tool the model may call.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolSpec {
    /// Stable tool name.
    pub name: String,
    /// Human/model readable description.
    pub description: String,
    /// JSON Schema for the arguments object.
    pub parameters: serde_json::Value,
}

/// Token and cost accounting for a single model call.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Usage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
    /// Provider-reported or estimated cost in USD, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

impl Usage {
    pub fn accumulate(&mut self, other: &Usage) {
        self.prompt_tokens += other.prompt_tokens;
        self.completion_tokens += other.completion_tokens;
        self.total_tokens += other.total_tokens;
        if let Some(c) = other.cost_usd {
            *self.cost_usd.get_or_insert(0.0) += c;
        }
    }
}
