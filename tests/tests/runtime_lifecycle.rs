//! Runtime lifecycle tests: cancellation, shutdown and crash recovery.

use std::sync::Arc;
use std::time::Duration;

use apex_core::error::Result;
use apex_memory::Store;
use apex_protocol::TaskStatus;
use apex_runtime::Runtime;
use apex_tests::support::{init_git_repo, offline_config};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_cancels_running_tasks_before_dropping_handles() -> Result<()> {
    // Regression test: shutdown previously removed each task handle before
    // signalling cancellation, so `cancel_task` found nothing running and the
    // executor never observed the cancel. Cancellation must be signalled first.
    let tmp = tempfile::tempdir()?;
    let repo = tmp.path().to_path_buf();
    init_git_repo(&repo).await?;

    let runtime = Runtime::new(offline_config(), Arc::new(Store::open_in_memory()?))?;
    let task = runtime.create_task(
        "Say hello".into(),
        repo.to_string_lossy().into_owned(),
        None,
        None,
        Default::default(),
    )?;
    assert!(runtime.is_running(&task.id), "task should be registered");

    // Grab the token the way shutdown does, then shut down.
    let token = runtime.cancel_token(&task.id);
    runtime.shutdown();

    assert!(
        token.is_cancelled(),
        "shutdown must signal cancellation to running task executors"
    );
    assert!(
        !runtime.is_running(&task.id),
        "task handles must be cleared after shutdown"
    );
    assert!(
        runtime.shutdown_token().is_cancelled(),
        "the runtime shutdown token must be set"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_a_running_task_sets_the_token() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let repo = tmp.path().to_path_buf();
    init_git_repo(&repo).await?;

    let runtime = Runtime::new(offline_config(), Arc::new(Store::open_in_memory()?))?;
    let task = runtime.create_task(
        "Say hello".into(),
        repo.to_string_lossy().into_owned(),
        None,
        None,
        Default::default(),
    )?;

    let token = runtime.cancel_token(&task.id);
    runtime.cancel_task(&task.id)?;
    assert!(
        token.is_cancelled(),
        "cancel_task must cancel the executor token"
    );

    // The executor observes the cancellation and marks the task cancelled.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let current = runtime.store.get_task(&task.id)?.expect("task exists");
        if current.status == TaskStatus::Cancelled {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "task did not reach Cancelled, last status was {:?}",
            current.status
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    runtime.shutdown();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_marks_in_flight_tasks_as_interrupted() -> Result<()> {
    // A task left in Running by a previous process must not be silently lost;
    // it is marked Failed with an explanatory error so the user can resume it.
    //
    // The task record is written directly rather than by spawning an executor:
    // recovery is about what a fresh process finds on disk, and this keeps the
    // test deterministic instead of racing a live task against a second SQLite
    // connection.
    let tmp = tempfile::tempdir()?;
    let db_path = tmp.path().join("apex.db");

    // Simulate the state left behind by a crashed process.
    {
        let store = Store::open(&db_path)?;
        let mut task = apex_memory::new_task(
            "Interrupted work",
            tmp.path().to_string_lossy().into_owned(),
            None,
            None,
        );
        task.status = TaskStatus::Running;
        store.create_task(&task)?;
    }

    // A fresh process must recover it.
    {
        let store = Arc::new(Store::open(&db_path)?);
        let _runtime = Runtime::new(offline_config(), Arc::clone(&store))?;

        let tasks = store.list_tasks(10)?;
        let recovered = tasks
            .into_iter()
            .find(|t| t.objective == "Interrupted work")
            .expect("the interrupted task should still exist");

        assert_eq!(
            recovered.status,
            TaskStatus::Failed,
            "interrupted tasks must be marked failed on restart"
        );
        assert!(
            recovered
                .error
                .as_deref()
                .is_some_and(|e| e.contains("interrupted")),
            "the error should explain that the task was interrupted, got {:?}",
            recovered.error
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn orchestrated_mode_requires_a_plan() -> Result<()> {
    // Orchestrated mode expects the orchestrator to have produced a plan. It
    // must fail with a clear error rather than silently degrading.
    let tmp = tempfile::tempdir()?;
    let runtime = Runtime::new(offline_config(), Arc::new(Store::open_in_memory()?))?;
    let err = runtime
        .create_task(
            "coordinate this".into(),
            tmp.path().to_string_lossy().into_owned(),
            None,
            None,
            apex_protocol::ExecutionMode::Orchestrated,
        )
        .expect_err("orchestrated mode needs a plan");
    assert!(matches!(err, apex_core::error::ApexError::Config(_)));
    assert!(
        err.to_string().contains("requires a plan"),
        "the error should explain what is missing, got: {err}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manual_multi_requires_a_team() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let runtime = Runtime::new(offline_config(), Arc::new(Store::open_in_memory()?))?;
    let err = runtime
        .create_task(
            "coordinate this".into(),
            tmp.path().to_string_lossy().into_owned(),
            None,
            None,
            apex_protocol::ExecutionMode::ManualMulti,
        )
        .expect_err("manual multi-agent mode needs a team");
    assert!(err.to_string().contains("requires a team"), "got: {err}");
    Ok(())
}
