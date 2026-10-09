//! Command implementations for the APEX CLI.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use apex_core::config::Config;
use apex_core::Project;
use apex_protocol::{ExecutionMode, Request, Response};

use crate::daemon;
use crate::render;
use crate::{AgentsCommand, Cli, Command, ModelsCommand, TaskCommand};

pub async fn dispatch(cli: Cli) -> Result<()> {
    let cwd = match &cli.cwd {
        Some(dir) => PathBuf::from(dir),
        None => std::env::current_dir()?,
    };
    let project = Project::discover(&cwd)?;
    let config = Config::load(Some(&project.root))?;

    match cli.command {
        Command::Runtime { foreground, stop } => {
            if stop {
                daemon::stop().await?;
                println!("APEX runtime stopped.");
                return Ok(());
            }
            if foreground {
                daemon::run_foreground(config).await?;
            } else {
                // Ensure a runtime is up, then report.
                let _client = daemon::connect_or_start(&config).await?;
                println!("APEX runtime is running.");
            }
            Ok(())
        }
        Command::Run {
            objective,
            model,
            agent,
            agents,
            parallel,
            writes,
            workflow,
            diff,
            no_wait,
        } => {
            let mut client = daemon::connect_or_start(&config).await?;
            // Resolve the execution mode and any team/workflow/plan up front so
            // a bad configuration fails before the task is created.
            let execution = build_execution(
                agents.as_deref(),
                parallel,
                writes.as_deref(),
                workflow.as_deref(),
            )?;
            run_task(
                &mut client,
                &objective,
                &project.root,
                model,
                agent,
                !no_wait,
                diff,
                cli.json,
                execution.mode,
                execution.team,
                execution.workflow,
                execution.plan,
            )
            .await
        }
        Command::Task(task_cmd) => {
            task_command(
                &mut daemon::connect_or_start(&config).await?,
                task_cmd,
                cli.json,
            )
            .await
        }
        Command::Agents(AgentsCommand::List) => {
            let mut client = daemon::connect_or_start(&config).await?;
            let response = client.request(Request::ListAgents).await?;
            if let Response::Agents { agents } = response {
                if cli.json {
                    println!("{}", serde_json::to_string_pretty(&agents)?);
                } else {
                    println!("{:<16} {:<10} NAME", "ID", "VERSION");
                    for agent in agents {
                        println!("{:<16} {:<10} {}", agent.id, agent.version, agent.name);
                    }
                }
            }
            Ok(())
        }
        Command::Agents(AgentsCommand::Run {
            agent_id,
            objective,
            model,
            no_wait,
        }) => {
            let mut client = daemon::connect_or_start(&config).await?;
            run_task(
                &mut client,
                &objective,
                &project.root,
                model,
                Some(agent_id),
                !no_wait,
                false,
                cli.json,
                ExecutionMode::Single,
                None,
                None,
                None,
            )
            .await
        }
        Command::Agents(AgentsCommand::Plan { workflow }) => {
            let definition =
                apex_orchestrator::WorkflowDefinition::load(std::path::Path::new(&workflow))?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&definition)?);
            } else {
                println!("workflow: {} — {}", definition.name, definition.description);
                for step in &definition.steps {
                    let deps = if step.depends_on.is_empty() {
                        String::new()
                    } else {
                        format!(" (after {})", step.depends_on.join(", "))
                    };
                    println!(
                        "  {:<12} {:<16} {}{}",
                        step.id, step.agent, step.objective, deps
                    );
                }
            }
            Ok(())
        }
        Command::Models(ModelsCommand::List) => {
            let mut config = config.clone();
            // Ensure a provider table exists even if empty.
            if config.providers.is_empty() {
                config.providers.insert(
                    "openai".into(),
                    apex_core::config::ProviderConfig::openai_compatible(
                        "https://api.openai.com/v1",
                    ),
                );
            }
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&config)?);
            } else {
                println!(
                    "default_model: {}",
                    config.default_model.as_deref().unwrap_or("(unset)")
                );
                for (name, provider) in &config.providers {
                    println!("\nprovider: {name} ({:?})", provider.kind);
                    println!("  base_url: {}", provider.base_url);
                    if let Some(env) = &provider.api_key_env {
                        let present = std::env::var(env).is_ok_and(|v| !v.is_empty());
                        println!(
                            "  api_key env: {env} ({})",
                            if present { "set" } else { "unset" }
                        );
                    }
                    println!("  models: {}", provider.models.join(", "));
                }
            }
            Ok(())
        }
        Command::Models(ModelsCommand::Set { reference }) => {
            let mut config = config.clone();
            let (provider, model) = reference.split_once('/').with_context(|| {
                format!("model reference must be provider/model, got '{reference}'")
            })?;
            if !config.providers.contains_key(provider) {
                // Seed a default OpenAI-compatible provider for the well-known names.
                let base = match provider {
                    "openai" => "https://api.openai.com/v1".to_string(),
                    _ => bail!(
                        "unknown provider '{provider}'. Add it to ~/.apex/config/config.toml first (see `apex models list`)."
                    ),
                };
                config.providers.insert(
                    provider.to_string(),
                    apex_core::config::ProviderConfig::openai_compatible(base),
                );
            }
            if let Some(p) = config.providers.get_mut(provider) {
                if !p.models.iter().any(|m| m == model) {
                    p.models.push(model.to_string());
                }
            }
            config.default_model = Some(reference.clone());
            config.save_user()?;
            println!("default_model set to {reference}");
            Ok(())
        }
        Command::Status => {
            let mut client = daemon::connect_or_start(&config).await?;
            let response = client.request(Request::Status).await?;
            if let Response::Status { status } = response {
                if cli.json {
                    println!("{}", serde_json::to_string_pretty(&status)?);
                } else {
                    println!("runtime:      v{}", status.runtime_version);
                    println!("protocol:     v{}", status.protocol_version);
                    println!("uptime:       {}s", status.uptime_secs);
                    println!(
                        "tasks:        {} total, {} active",
                        status.task_count, status.active_task_count
                    );
                    println!("database:     {}", status.database_path);
                }
            }
            Ok(())
        }
        Command::Tui => crate::tui::run(config).await,
        Command::Diff { task_id } => {
            let mut client = daemon::connect_or_start(&config).await?;
            let task_id = resolve_task_id(&mut client, task_id).await?;
            let response = client.request(Request::GetDiff { task_id }).await?;
            if let Response::Diff { diff } = response {
                if cli.json {
                    println!("{}", serde_json::to_string(&diff)?);
                } else {
                    println!("{diff}");
                }
            }
            Ok(())
        }
        Command::Verify { task_id } => {
            let mut client = daemon::connect_or_start(&config).await?;
            let task_id = resolve_task_id(&mut client, task_id).await?;
            let response = client.request(Request::Verify { task_id }).await?;
            if let Response::Verification { passed, checks } = response {
                if cli.json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(
                            &serde_json::json!({"passed": passed, "checks": checks})
                        )?
                    );
                } else {
                    for check in &checks {
                        let mark = if check.passed { "PASS" } else { "FAIL" };
                        println!("[{mark}] {}: {}", check.name, first_line(&check.detail));
                    }
                    println!("verification {}", if passed { "passed" } else { "failed" });
                }
            }
            Ok(())
        }
        Command::Config => {
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&config)?);
            } else {
                println!("# effective configuration for {}", project.root.display());
                println!("{}", toml::to_string_pretty(&config)?);
            }
            Ok(())
        }
        Command::Doctor => doctor(&project).await,
    }
}

/// The execution mode and everything that configures it.
struct Execution {
    mode: ExecutionMode,
    team: Option<apex_protocol::TeamSpec>,
    workflow: Option<apex_orchestrator::WorkflowDefinition>,
    plan: Option<apex_protocol::Plan>,
}

/// Build the execution mode and its team / workflow / plan configuration.
///
/// Exactly one of single-agent, manual multi-agent or workflow may be selected;
/// anything ambiguous is rejected rather than silently guessed.
fn build_execution(
    agents: Option<&str>,
    parallel: bool,
    writes: Option<&str>,
    workflow: Option<&str>,
) -> Result<Execution> {
    use apex_orchestrator::WorkflowDefinition;
    use apex_protocol::{TeamSpec, TeamStrategy};

    if agents.is_some() && workflow.is_some() {
        anyhow::bail!("--agents and --workflow cannot be combined; pick one");
    }
    if parallel && agents.is_none() {
        anyhow::bail!("--parallel only applies with --agents");
    }

    if let Some(file) = workflow {
        let definition = WorkflowDefinition::load(std::path::Path::new(file))?;
        return Ok(Execution {
            mode: ExecutionMode::Workflow,
            team: None,
            workflow: Some(definition),
            plan: None,
        });
    }

    if let Some(csv) = agents {
        let mut spec = TeamSpec::from_csv(csv);
        spec.strategy = if parallel {
            TeamStrategy::Parallel
        } else {
            TeamStrategy::Sequential
        };
        if let Some(globs) = writes {
            let paths: Vec<String> = globs
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
            if paths.is_empty() {
                anyhow::bail!("--writes needs at least one glob, e.g. --writes 'src/**'");
            }
            spec.writes = Some(paths);
        }
        return Ok(Execution {
            mode: ExecutionMode::ManualMulti,
            team: Some(spec),
            workflow: None,
            plan: None,
        });
    }

    Ok(Execution {
        mode: ExecutionMode::Single,
        team: None,
        workflow: None,
        plan: None,
    })
}

/// Create a task and (optionally) stream it to completion.
#[allow(clippy::too_many_arguments)]
async fn run_task(
    client: &mut apex_runtime::client::Client,
    objective: &str,
    project_root: &std::path::Path,
    model: Option<String>,
    agent: Option<String>,
    wait: bool,
    show_diff: bool,
    json: bool,
    mode: ExecutionMode,
    team: Option<apex_protocol::TeamSpec>,
    workflow: Option<apex_orchestrator::WorkflowDefinition>,
    plan: Option<apex_protocol::Plan>,
) -> Result<()> {
    let root = project_root.to_string_lossy().into_owned();
    let response = client
        .request(Request::CreateTask {
            objective: objective.to_string(),
            project_root: root,
            model,
            agent_id: agent,
            mode,
            team,
            workflow,
            plan: plan.map(|p| apex_protocol::PlannedTask { steps: p.steps }),
        })
        .await?;
    let Response::Task { task } = response else {
        bail!("unexpected response creating task");
    };
    if json && !wait {
        println!("{}", serde_json::to_string_pretty(&task)?);
        return Ok(());
    }
    if !json {
        println!(
            "task {} started (resume with `apex task resume {}`)",
            task.id, task.id
        );
    }

    if wait {
        stream_until_terminal(client, &task.id, json).await?;
        // Final report.
        let Response::Task { task } = client
            .request(Request::ShowTask {
                task_id: task.id.clone(),
            })
            .await?
        else {
            bail!("unexpected response fetching task");
        };
        let verification = match client
            .request(Request::Verify {
                task_id: task.id.clone(),
            })
            .await?
        {
            Response::Verification { passed, checks } => Some((passed, checks)),
            _ => None,
        };
        if json {
            let mut value = serde_json::to_value(&task)?;
            value["verification"] = serde_json::to_value(&verification)?;
            println!("{}", serde_json::to_string_pretty(&value)?);
        } else {
            print!("{}", render::render_report(&task, verification));
        }
        if show_diff {
            if let Response::Diff { diff } = client
                .request(Request::GetDiff {
                    task_id: task.id.clone(),
                })
                .await?
            {
                println!("\n=== Diff ===\n{diff}");
            }
        }
    }
    Ok(())
}

/// Poll events until the task reaches a terminal state, rendering progress.
async fn stream_until_terminal(
    client: &mut apex_runtime::client::Client,
    task_id: &str,
    json: bool,
) -> Result<()> {
    let mut after = 0i64;
    loop {
        let response = client
            .request(Request::TaskEvents {
                task_id: task_id.to_string(),
                after_seq: Some(after),
            })
            .await?;
        let Response::Events { events, .. } = response else {
            bail!("unexpected response fetching events");
        };
        for event in events.iter() {
            after = event.seq;
            if json {
                println!("{}", serde_json::to_string(event)?);
            } else {
                let line = render::render_event(event);
                if !line.trim().is_empty() {
                    println!("{line}");
                }
            }
        }
        // Check terminal status.
        let Response::Task { task } = client
            .request(Request::ShowTask {
                task_id: task_id.to_string(),
            })
            .await?
        else {
            bail!("unexpected response fetching task");
        };
        if task.status.is_terminal() {
            return Ok(());
        }
        // No new events yet: pause briefly before polling again so we do not
        // spin the runtime while the agent is thinking.
        if events.is_empty() {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
}

/// Task subcommands.
async fn task_command(
    client: &mut apex_runtime::client::Client,
    cmd: TaskCommand,
    json: bool,
) -> Result<()> {
    match cmd {
        TaskCommand::List { limit } => {
            let response = client
                .request(Request::ListTasks { limit: Some(limit) })
                .await?;
            if let Response::TaskList { tasks } = response {
                if json {
                    println!("{}", serde_json::to_string_pretty(&tasks)?);
                } else {
                    println!("{:<22} {:<12} {:<10} OBJECTIVE", "ID", "STATUS", "PROJECT");
                    for task in tasks.iter() {
                        println!("{}", render::render_task_line(task));
                    }
                }
            }
            Ok(())
        }
        TaskCommand::Show { task_id, subtasks } => {
            let Response::Task { task } = client
                .request(Request::ShowTask {
                    task_id: task_id.clone(),
                })
                .await?
            else {
                bail!("unexpected response");
            };
            if json {
                println!("{}", serde_json::to_string_pretty(&task)?);
            } else {
                let Response::Events { events, .. } = client
                    .request(Request::TaskEvents {
                        task_id: task_id.clone(),
                        after_seq: None,
                    })
                    .await?
                else {
                    bail!("unexpected response");
                };
                for event in events.iter() {
                    let line = render::render_event(event);
                    if !line.trim().is_empty() {
                        println!("{line}");
                    }
                }
                print!("{}", render::render_report(&task, None));
            }
            if subtasks {
                print_subtasks(client, &task_id, json).await?;
            }
            Ok(())
        }
        TaskCommand::Subtasks { task_id } => print_subtasks(client, &task_id, json).await,
        TaskCommand::Plan { task_id } => {
            let response = client.request(Request::ShowPlan { task_id }).await?;
            if let Response::Plan { plan, waves, .. } = response {
                if json {
                    println!("{}", serde_json::to_string_pretty(&plan)?);
                } else if plan.is_empty() {
                    println!("this task has no multi-step plan (single-agent mode)");
                } else {
                    println!("{:<14} {:<5} {:<16} OBJECTIVE", "STEP", "WAVE", "AGENT");
                    for (index, step) in plan.steps.iter().enumerate() {
                        let wave = waves.get(index).copied().unwrap_or(0);
                        println!(
                            "{:<14} {:<5} {:<16} {}",
                            step.id,
                            wave,
                            step.agent_id,
                            render::truncate(&step.objective, 60)
                        );
                    }
                }
            }
            Ok(())
        }
        TaskCommand::Resume { task_id } => {
            client
                .request(Request::ResumeTask {
                    task_id: task_id.clone(),
                })
                .await?;
            println!("task {task_id} resuming");
            stream_until_terminal(client, &task_id, json).await?;
            Ok(())
        }
        TaskCommand::Send {
            task_id,
            instruction,
        } => {
            client
                .request(Request::SendInstruction {
                    task_id: task_id.clone(),
                    instruction,
                })
                .await?;
            println!("instruction sent to {task_id}");
            stream_until_terminal(client, &task_id, json).await?;
            Ok(())
        }
        TaskCommand::Cancel { task_id } => {
            client
                .request(Request::CancelTask {
                    task_id: task_id.clone(),
                })
                .await?;
            println!("task {task_id} cancelled");
            Ok(())
        }
    }
}

/// Print the subtasks of a multi-agent task.
async fn print_subtasks(
    client: &mut apex_runtime::client::Client,
    task_id: &str,
    json: bool,
) -> Result<()> {
    let response = client
        .request(Request::ListSubtasks {
            task_id: task_id.to_string(),
        })
        .await?;
    let Response::SubtaskList { subtasks, .. } = response else {
        anyhow::bail!("unexpected response listing subtasks");
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&subtasks)?);
        return Ok(());
    }
    if subtasks.is_empty() {
        println!("(no subtasks — this was a single-agent task)");
        return Ok(());
    }
    println!(
        "{:<14} {:<5} {:<16} {:<10} RESULT",
        "SUBTASK", "WAVE", "AGENT", "STATUS"
    );
    for subtask in subtasks.iter() {
        let result = subtask
            .result
            .as_deref()
            .or(subtask.error.as_deref())
            .unwrap_or("");
        println!(
            "{:<14} {:<5} {:<16} {:<10} {}",
            subtask.id,
            subtask.wave,
            subtask.agent_id,
            subtask.status.as_str(),
            render::truncate(result.trim(), 60)
        );
    }
    Ok(())
}

/// Resolve a task id, defaulting to the most recent task.
async fn resolve_task_id(
    client: &mut apex_runtime::client::Client,
    task_id: Option<String>,
) -> Result<String> {
    if let Some(id) = task_id {
        return Ok(id);
    }
    let response = client
        .request(Request::ListTasks { limit: Some(1) })
        .await?;
    if let Response::TaskList { tasks } = response {
        if let Some(task) = tasks.first() {
            return Ok(task.id.clone());
        }
    }
    bail!("no tasks found; specify a task id")
}

/// Environment diagnostics.
async fn doctor(project: &Project) -> Result<()> {
    println!("APEX doctor\n============");
    println!("apex version:  {}", env!("CARGO_PKG_VERSION"));
    println!("project root:  {}", project.root.display());
    println!(
        "git repo:      {}",
        if project.is_git { "yes" } else { "no" }
    );

    let git_ok = apex_core::git::is_git_available().await;
    println!(
        "git binary:    {}",
        if git_ok { "available" } else { "NOT FOUND" }
    );

    let home = apex_core::paths::apex_home()?;
    println!("apex home:     {}", home.display());
    let db = apex_core::paths::database_path()?;
    println!(
        "database:      {} ({})",
        db.display(),
        if db.exists() {
            "present"
        } else {
            "will be created"
        }
    );

    match daemon::try_connect().await {
        Ok(Some(_)) => println!("runtime:       running"),
        Ok(None) => println!("runtime:       not running (will autostart)"),
        Err(e) => println!("runtime:       error: {e}"),
    }

    let config = Config::load(Some(&project.root))?;
    println!("\nmodel:");
    match &config.default_model {
        Some(m) => {
            let provider = m.split('/').next().unwrap_or("");
            println!("  default: {m}");
            match config.providers.get(provider) {
                Some(p) => {
                    let key_status = p
                        .api_key_env
                        .as_ref()
                        .map(|v| {
                            if std::env::var(v).is_ok_and(|k| !k.is_empty()) {
                                "key set"
                            } else {
                                "KEY MISSING"
                            }
                        })
                        .unwrap_or("no key env");
                    println!("  provider: {} ({:?}) at {}", provider, p.kind, p.base_url);
                    println!("  api key:  {key_status}");
                }
                None => println!("  provider '{provider}' is NOT configured"),
            }
        }
        None => println!("  default:  (unset) — run `apex models set provider/model`"),
    }
    Ok(())
}

fn first_line(text: &str) -> String {
    render::truncate(text.lines().next().unwrap_or(""), 120)
}
