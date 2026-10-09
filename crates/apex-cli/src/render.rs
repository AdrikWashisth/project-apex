//! Human-readable rendering of tasks, events and reports.

use apex_protocol::{EventKind, Task, TaskStatus};

/// Truncate a single-line preview of tool arguments.
fn preview(value: &serde_json::Value, max: usize) -> String {
    let text = value.to_string();
    if text.len() <= max {
        text
    } else {
        format!("{}…", &text[..max])
    }
}

/// Render one streamed event as a line of terminal output.
pub fn render_event(event: &apex_protocol::Event) -> String {
    match &event.kind {
        EventKind::TaskCreated { objective, .. } => {
            format!("▶ task started: {objective}")
        }
        EventKind::StatusChanged { status, reason } => match status {
            TaskStatus::Running => format!(
                "… running{}",
                reason
                    .as_deref()
                    .map(|r| format!(" ({r})"))
                    .unwrap_or_default()
            ),
            TaskStatus::Verifying => "… verifying".to_string(),
            TaskStatus::Completed => "✓ completed".to_string(),
            TaskStatus::Failed => "✗ failed".to_string(),
            TaskStatus::Cancelled => "⊘ cancelled".to_string(),
            _ => format!("… {status:?}"),
        },
        EventKind::Message { message } => {
            let text = message.content.clone().unwrap_or_default();
            if message.tool_calls.is_empty() {
                text.trim().to_string()
            } else {
                String::new()
            }
        }
        EventKind::ToolStarted {
            name, arguments, ..
        } => {
            format!("  ⚙ {name} {}", preview(arguments, 120))
        }
        EventKind::ToolFinished {
            name,
            success,
            summary,
            ..
        } => {
            let mark = if *success { "✓" } else { "✗" };
            format!("  {mark} {name}: {summary}")
        }
        EventKind::Verification { passed, checks } => {
            let passed_n = checks.iter().filter(|c| c.passed).count();
            format!(
                "  {} verification: {}/{} checks passed",
                if *passed { "✓" } else { "✗" },
                passed_n,
                checks.len()
            )
        }
        EventKind::Error { message } => format!("  ✗ {message}"),
        EventKind::Log { level, message } => format!("  [{level}] {message}"),
        _ => String::new(),
    }
}

/// Render a short task line for listings.
pub fn render_task_line(task: &Task) -> String {
    let status = match task.status {
        TaskStatus::Completed => "completed",
        TaskStatus::Failed => "failed",
        TaskStatus::Cancelled => "cancelled",
        TaskStatus::Running => "running",
        TaskStatus::Verifying => "verifying",
        TaskStatus::WaitingApproval => "awaiting-approval",
        TaskStatus::Pending => "pending",
    };
    format!(
        "{:<22} {:<12} {:<10} {}",
        task.id,
        status,
        task.project_root
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(&task.project_root),
        truncate(&task.objective, 60)
    )
}

/// Render the final completion report.
pub fn render_report(
    task: &Task,
    verification: Option<(bool, Vec<apex_protocol::CheckResult>)>,
) -> String {
    let mut out = String::new();
    let status = match task.status {
        TaskStatus::Completed => "COMPLETED",
        TaskStatus::Failed => "FAILED",
        TaskStatus::Cancelled => "CANCELLED",
        other => other.as_str(),
    };
    out.push_str(&format!("\n=== Task {}: {} ===\n", task.id, status));
    if let Some(summary) = &task.summary {
        out.push_str(&format!("\n{}\n", summary.trim()));
    }
    if let Some((passed, checks)) = &verification {
        out.push_str("\nVerification:\n");
        for check in checks {
            let mark = if check.passed { "PASS" } else { "FAIL" };
            out.push_str(&format!("  [{mark}] {}\n", check.name));
        }
        let _ = passed;
    }
    out.push_str(&format!(
        "\nEvidence: {} tool call(s), {} step(s), {} repair attempt(s), {} tokens\n",
        task.tool_calls, task.steps, task.repair_attempts, task.usage.total_tokens
    ));
    if let Some(err) = &task.error {
        out.push_str(&format!("Error: {err}\n"));
    }
    out
}

/// Truncate to a maximum length.
pub fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let truncated: String = text.chars().take(max).collect();
        format!("{truncated}…")
    }
}
