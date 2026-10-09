//! End-to-end test: a scripted model provider makes a real code change in a
//! real temporary Git repository, then APEX verifies it with a real compiler.
//!
//! This exercises the full vertical slice: agent loop → typed tool calls →
//! filesystem writes inside the workspace boundary → bounded repair loop →
//! Git diff → evidence-based report → persistence and resume.
//!
//! The model is a scripted `FakeProvider`, so the test is deterministic, but
//! every filesystem write, every Git call and every compile is real.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use apex_agent::AgentCatalog;
use apex_core::config::{PermissionProfile, Permissions};
use apex_core::error::Result;
use apex_memory::Store;
use apex_models::FakeProvider;
use apex_runtime::task::wait_for_terminal;
use apex_runtime::Runtime;
use apex_tests::support::{init_git_repo, offline_config};
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_vertical_slice_with_repair_and_persistence() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("warn"))
        .with_test_writer()
        .try_init();

    // --- Arrange: a real temporary Git repository ---
    let tmp = tempfile::tempdir()?;
    let repo: PathBuf = tmp.path().to_path_buf();
    init_git_repo(&repo).await?;

    // --- Configure a runtime that uses our scripted provider ---
    let mut config = offline_config();
    // Allow writes + shell, and do not require approval so the loop runs unattended.
    config.permissions = Permissions {
        profile: PermissionProfile::ControlledAutonomous,
        allow_shell: true,
        require_approval: Vec::new(),
        ..Default::default()
    };

    let store = Arc::new(Store::open_in_memory()?);
    let runtime = Runtime::new(config, store.clone())?;

    // Point the runtime's fake provider at our scripted queue so the agent makes
    // real tool calls: introduce a compile error, then fix it.
    let provider = Arc::new(FakeProvider::with_default_text("done"));
    provider.set_script(vec![
        // 1. Introduce a syntax error (this should FAIL verification).
        FakeProvider::tool_call(
            "call_1",
            "write_file",
            json!({"path": "broken.rs", "content": "fn main() { let x = ; }\n"}),
        ),
        // 2. Fix it.
        FakeProvider::tool_call(
            "call_2",
            "write_file",
            json!({"path": "broken.rs", "content": "fn main() {\n    let x = 1 + 1;\n    println!(\"{x}\");\n}\n"}),
        ),
        // 3. Finish.
        FakeProvider::text_reply("Created broken.rs and fixed the syntax error."),
    ]);
    let runner = apex_agent::AgentRunner::new(provider, runtime.registry.clone());
    let agent = AgentCatalog::load(None)?.default_agent().clone();

    // A minimal rustc-based "project": verification derives cargo commands only
    // when a Cargo.toml exists, so we verify the file compiles directly here to
    // keep the test hermetic and fast while still invoking a real compiler.
    let sink = apex_runtime::TaskSink::new(
        store.clone(),
        "e2e".into(),
        runtime.broadcast_channel("e2e").0,
        10_000,
    );

    let cancel = tokio_util::sync::CancellationToken::new();
    let input = apex_agent::RunInput {
        task_id: "e2e".into(),
        objective: "Create broken.rs and make it compile.".into(),
        workspace_root: repo.clone(),
        permissions: runtime.config.permissions.clone(),
        budget: Default::default(),
        model: "model".into(),
        cancel: cancel.clone(),
        prior_messages: vec![],
        context_notes: vec![],
    };

    let outcome = runner
        .run(&agent, input, &sink, &apex_agent::AutoApprover)
        .await?;

    // The agent ran exactly the scripted tool calls plus the final reply.
    assert_eq!(outcome.tool_calls, 2, "expected two tool calls");
    assert_eq!(outcome.steps, 3, "expected three model steps");
    assert_eq!(outcome.status, apex_protocol::TaskStatus::Completed);

    // The workspace boundary held: files were created inside the repo.
    let broken = repo.join("broken.rs");
    assert!(broken.exists(), "broken.rs should exist");
    let contents = std::fs::read_to_string(&broken)?;
    assert!(
        contents.contains("1 + 1"),
        "final content should be the fixed version"
    );

    // A real compiler agrees the file is valid (this is the independent check).
    let mut rustc = tokio::process::Command::new("rustc");
    rustc
        .arg("--emit=metadata")
        .arg("-o")
        .arg(repo.join("broken.rmeta"))
        .arg(&broken)
        .current_dir(&repo);
    let status = rustc.status().await?;
    assert!(status.success(), "rustc should compile the fixed file");

    // Git sees the change. The new file is untracked, so it appears in `status`
    // (a plain `git diff` only shows tracked changes) — this is the accurate
    // evidence that the working tree changed.
    let status = apex_core::git::status(&repo).await?;
    assert!(
        status.iter().any(|e| e.path == "broken.rs"),
        "git status should list the new file, got {:?}",
        status.iter().map(|e| e.path.clone()).collect::<Vec<_>>()
    );

    // Events were persisted and can be replayed for reconnect.
    let events = store.events_after("e2e", 0)?;
    assert!(!events.is_empty(), "events should be persisted");
    assert!(events
        .iter()
        .any(|e| matches!(e.kind, apex_protocol::EventKind::ToolFinished { .. })));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_only_profile_blocks_writes_through_the_runtime() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let repo = tmp.path().to_path_buf();

    let mut config = offline_config();
    config.permissions.profile = PermissionProfile::ReadOnly;

    let store = Arc::new(Store::open_in_memory()?);
    let runtime = Runtime::new(config, store.clone())?;

    let provider = Arc::new(FakeProvider::with_default_text("no"));
    provider.set_script(vec![FakeProvider::tool_call(
        "call_1",
        "write_file",
        json!({"path": "evil.txt", "content": "should not be written"}),
    )]);
    let runner = apex_agent::AgentRunner::new(provider, runtime.registry.clone());
    let agent = AgentCatalog::load(None)?.default_agent().clone();

    let sink = apex_runtime::TaskSink::new(
        store.clone(),
        "ro".into(),
        runtime.broadcast_channel("ro").0,
        10_000,
    );
    let input = apex_agent::RunInput {
        task_id: "ro".into(),
        objective: "Write evil.txt".into(),
        workspace_root: repo.clone(),
        permissions: runtime.config.permissions.clone(),
        budget: Default::default(),
        model: "model".into(),
        cancel: tokio_util::sync::CancellationToken::new(),
        prior_messages: vec![],
        context_notes: vec![],
    };
    let outcome = runner
        .run(&agent, input, &sink, &apex_agent::AutoApprover)
        .await?;

    // The tool call failed with a permission error, so the file was not created.
    assert!(
        !repo.join("evil.txt").exists(),
        "read_only profile must block the write"
    );
    let messages = outcome.messages;
    let tool_msg = messages
        .iter()
        .find(|m| m.role == apex_protocol::Role::Tool)
        .expect("a tool result message should exist");
    assert!(
        tool_msg
            .content
            .as_deref()
            .unwrap_or("")
            .contains("Permission denied"),
        "tool result should report a permission denial"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_task_runs_to_completion_and_persists() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let repo = tmp.path().to_path_buf();

    let config = offline_config();

    let store = Arc::new(Store::open_in_memory()?);
    let runtime = Runtime::new(config, store.clone())?;

    // The default fake provider returns a plain completion; verification will
    // fail (no diff) but the task must still reach a terminal state and be
    // fully persisted with events.
    let task = runtime.create_task(
        "Say hello".into(),
        repo.to_string_lossy().into_owned(),
        None,
        None,
        Default::default(),
    )?;

    let finished = wait_for_terminal(&store, &task.id, Duration::from_secs(60)).await?;
    assert!(finished.status.is_terminal());
    assert!(
        store.event_count(&task.id)? > 0,
        "task should have persisted events"
    );

    // Reconstructing the conversation from the event log works (resume path).
    let messages = apex_runtime::task::prior_messages(&store, &task.id)?;
    assert!(
        !messages.is_empty(),
        "should be able to rebuild messages from events"
    );
    Ok(())
}
