//! The bounded agent execution loop.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use apex_core::config::{Budget, Permissions, RiskClass};
use apex_core::error::{ApexError, Result};
use apex_models::{CompletionRequest, ModelProvider};
use apex_protocol::{ChatMessage, EventKind, EventSink, TaskStatus, Usage};
use apex_tools::{ToolContext, ToolRegistry};
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::manifest::AgentManifest;

/// A request for human approval of a risky action.
#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    pub task_id: String,
    pub approval_id: String,
    pub action: String,
    pub risk: RiskClass,
    pub detail: String,
}

/// Decides whether a gated action may proceed.
#[async_trait]
pub trait Approver: Send + Sync {
    async fn approve(&self, request: &ApprovalRequest) -> Result<bool>;
}

/// Approves every gated action. Used when the active profile allows it.
pub struct AutoApprover;

#[async_trait]
impl Approver for AutoApprover {
    async fn approve(&self, _request: &ApprovalRequest) -> Result<bool> {
        Ok(true)
    }
}

/// Denies every gated action. Used for conservative or unattended runs.
pub struct DenyAllApprover;

#[async_trait]
impl Approver for DenyAllApprover {
    async fn approve(&self, _request: &ApprovalRequest) -> Result<bool> {
        Ok(false)
    }
}

/// Everything the runner needs to execute a task.
#[derive(Clone)]
pub struct RunInput {
    pub task_id: String,
    pub objective: String,
    pub workspace_root: PathBuf,
    pub permissions: Permissions,
    pub budget: Budget,
    pub model: String,
    pub cancel: CancellationToken,
    /// Prior conversation, used when resuming.
    pub prior_messages: Vec<ChatMessage>,
    /// Additional context injected into the system prompt.
    pub context_notes: Vec<String>,
}

/// The result of an agent run.
pub struct AgentOutcome {
    pub status: TaskStatus,
    pub summary: String,
    pub final_text: Option<String>,
    pub messages: Vec<ChatMessage>,
    pub usage: Usage,
    pub tool_calls: u32,
    pub steps: u32,
    pub error: Option<String>,
}

/// Executes agent manifests against a model and tool registry.
pub struct AgentRunner {
    provider: Arc<dyn ModelProvider>,
    registry: Arc<ToolRegistry>,
}

impl AgentRunner {
    pub fn new(provider: Arc<dyn ModelProvider>, registry: Arc<ToolRegistry>) -> Self {
        AgentRunner { provider, registry }
    }

    /// Run the agent loop to completion, a budget limit, or cancellation.
    pub async fn run(
        &self,
        agent: &AgentManifest,
        input: RunInput,
        sink: &dyn EventSink,
        approver: &dyn Approver,
    ) -> Result<AgentOutcome> {
        let started = Instant::now();
        let budget = &input.budget;
        let cancel = input.cancel.clone();

        let system = build_system_prompt(agent, &input);
        let mut messages: Vec<ChatMessage> = Vec::new();
        messages.push(ChatMessage::system(system));
        messages.extend(input.prior_messages.clone());
        messages.push(ChatMessage::user(input.objective.clone()));

        let tool_specs = self.tool_specs(agent);
        let ctx = ToolContext::new(input.workspace_root.clone(), input.permissions.clone());

        let mut usage = Usage::default();
        let mut steps: u32 = 0;
        let mut tool_calls: u32 = 0;
        let mut final_text: Option<String> = None;

        loop {
            if cancel.is_cancelled() {
                return Err(ApexError::Cancelled);
            }
            if steps >= budget.max_steps {
                return Ok(AgentOutcome {
                    status: TaskStatus::Failed,
                    summary: format!("step budget exhausted ({} steps)", budget.max_steps),
                    final_text,
                    messages,
                    usage,
                    tool_calls,
                    steps,
                    error: Some("step limit reached".into()),
                });
            }
            if started.elapsed().as_secs() > budget.max_wall_secs {
                return Err(ApexError::Budget(format!(
                    "wall-clock budget of {}s exceeded",
                    budget.max_wall_secs
                )));
            }
            if usage.total_tokens > budget.max_model_tokens {
                return Err(ApexError::Budget(format!(
                    "token budget of {} exceeded",
                    budget.max_model_tokens
                )));
            }
            if let Some(cost) = usage.cost_usd {
                if cost > budget.max_cost_usd {
                    return Err(ApexError::Budget(format!(
                        "cost budget of ${:.2} exceeded (${:.2} spent)",
                        budget.max_cost_usd, cost
                    )));
                }
            }

            steps += 1;

            let request = CompletionRequest::new(input.model.clone(), messages.clone())
                .with_tools(tool_specs.clone());
            let response = self.provider.complete(request).await?;
            usage.accumulate(&response.usage);
            sink.emit(EventKind::Usage {
                usage: response.usage.clone(),
            });

            let assistant = response.message.clone();
            messages.push(assistant.clone());
            sink.emit(EventKind::Message {
                message: assistant.clone(),
            });

            let calls = assistant.tool_calls.clone();
            if calls.is_empty() {
                final_text = assistant.content.clone();
                break;
            }

            if let Some(text) = &assistant.content {
                if !text.trim().is_empty() {
                    // The model narrated before acting; already emitted above.
                }
            }

            for call in calls {
                if cancel.is_cancelled() {
                    return Err(ApexError::Cancelled);
                }
                if tool_calls >= budget.max_tool_calls {
                    return Err(ApexError::Budget(format!(
                        "tool-call budget of {} exceeded",
                        budget.max_tool_calls
                    )));
                }
                tool_calls += 1;

                sink.emit(EventKind::ToolStarted {
                    call_id: call.id.clone(),
                    name: call.name.clone(),
                    arguments: call.arguments.clone(),
                });

                let result_text = self.execute_call(&call, &ctx, &input, sink, approver).await;

                let (ok, summary, content) = match result_text {
                    Ok((ok, summary, content)) => (ok, summary, content),
                    Err(e) => (false, format!("tool error: {e}"), format!("Error: {e}")),
                };

                sink.emit(EventKind::ToolFinished {
                    call_id: call.id.clone(),
                    name: call.name.clone(),
                    success: ok,
                    summary: summary.clone(),
                });

                messages.push(ChatMessage::tool_result(
                    call.id.clone(),
                    call.name.clone(),
                    content,
                ));
            }
        }

        let summary = final_text
            .clone()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| "Agent finished without a final message.".into());

        Ok(AgentOutcome {
            status: TaskStatus::Completed,
            summary,
            final_text,
            messages,
            usage,
            tool_calls,
            steps,
            error: None,
        })
    }

    async fn execute_call(
        &self,
        call: &apex_protocol::ToolCall,
        ctx: &ToolContext,
        input: &RunInput,
        sink: &dyn EventSink,
        approver: &dyn Approver,
    ) -> Result<(bool, String, String)> {
        let risk = self.registry.risk(&call.name);
        if let Some(risk) = risk {
            if ctx.needs_approval(risk) {
                let approval_id = apex_core::new_id("appr");
                sink.emit(EventKind::ApprovalRequested {
                    approval_id: approval_id.clone(),
                    action: call.name.clone(),
                    risk: format!("{risk:?}"),
                    detail: call.arguments.to_string(),
                });
                let request = ApprovalRequest {
                    task_id: input.task_id.clone(),
                    approval_id: approval_id.clone(),
                    action: call.name.clone(),
                    risk,
                    detail: call.arguments.to_string(),
                };
                let approved = approver.approve(&request).await.unwrap_or(false);
                sink.emit(EventKind::ApprovalResolved {
                    approval_id,
                    approved,
                });
                if !approved {
                    let msg = format!("Action '{}' was denied by policy.", call.name);
                    return Ok((false, "denied by policy".into(), msg));
                }
            }
        }

        match self
            .registry
            .execute(&call.name, call.arguments.clone(), ctx)
            .await
        {
            Ok(result) => {
                let content = if result.ok {
                    result.content
                } else {
                    format!(
                        "Tool reported failure: {}\n{}",
                        result.summary, result.content
                    )
                };
                Ok((result.ok, result.summary, content))
            }
            Err(ApexError::Permission(msg)) => {
                let content = format!("Permission denied: {msg}");
                Ok((false, "permission denied".into(), content))
            }
            Err(e) => Err(e),
        }
    }

    fn tool_specs(&self, agent: &AgentManifest) -> Vec<apex_protocol::ToolSpec> {
        self.registry
            .specs()
            .into_iter()
            .filter(|spec| agent.allows_tool(&spec.name))
            .collect()
    }
}

fn build_system_prompt(agent: &AgentManifest, input: &RunInput) -> String {
    let mut prompt = String::new();
    prompt.push_str(agent.instructions.trim());
    prompt.push_str(&format!(
        "\n\nWorkspace root: {}\n",
        input.workspace_root.display()
    ));
    prompt.push_str(&format!("Agent: {} v{}\n", agent.name, agent.version));
    if !input.context_notes.is_empty() {
        prompt.push_str("\nProject memory and notes:\n");
        for note in &input.context_notes {
            prompt.push_str(&format!("- {note}\n"));
        }
    }
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::default_agent;
    use apex_core::config::Permissions;
    use apex_models::FakeProvider;
    use apex_protocol::NullSink;
    use serde_json::json;

    fn input(dir: PathBuf) -> RunInput {
        RunInput {
            task_id: "task_test".into(),
            objective: "say hello".into(),
            workspace_root: dir,
            permissions: Permissions::default(),
            budget: Default::default(),
            model: "fake".into(),
            cancel: CancellationToken::new(),
            prior_messages: vec![],
            context_notes: vec![],
        }
    }

    #[tokio::test]
    async fn finishes_with_text() {
        let provider = Arc::new(FakeProvider::with_default_text("all done"));
        let runner = AgentRunner::new(provider, Arc::new(ToolRegistry::default_set()));
        let outcome = runner
            .run(
                &default_agent(),
                input(std::env::temp_dir()),
                &NullSink,
                &AutoApprover,
            )
            .await
            .unwrap();
        assert_eq!(outcome.status, TaskStatus::Completed);
        assert_eq!(outcome.steps, 1);
        assert!(outcome.summary.contains("all done"));
    }

    #[tokio::test]
    async fn executes_a_tool_call_then_finishes() {
        let provider = Arc::new(FakeProvider::with_default_text("finished"));
        provider.set_script(vec![
            FakeProvider::tool_call("call_1", "project_info", json!({})),
            FakeProvider::text_reply("finished"),
        ]);
        let runner = AgentRunner::new(provider, Arc::new(ToolRegistry::default_set()));
        let outcome = runner
            .run(
                &default_agent(),
                input(std::env::temp_dir()),
                &NullSink,
                &AutoApprover,
            )
            .await
            .unwrap();
        assert_eq!(outcome.tool_calls, 1);
        assert_eq!(outcome.steps, 2);
        assert_eq!(outcome.status, TaskStatus::Completed);
    }
}
