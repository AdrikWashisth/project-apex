//! Command implementations for the APEX CLI.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use apex_core::config::Config;
use apex_core::Project;
use apex_protocol::{Request, Response};

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
            diff,
            no_wait,
        } => {
            let mut client = daemon::connect_or_start(&config).await?;
            run_task(
                &mut client,
                &objective,
                &project.root,
                model,
                agent,
                !no_wait,
                diff,
                cli.json,
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
                    println!("{:<16} {:<10} {}", "ID", "VERSION", "NAME");
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
            )
            .await
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

/// Create a task and (optionally) stream it to completion.
#[allow(clippy::too_many_arguments)]
async fn run_task(
    client: &mut apex_runtime::client::Client,
    objective: &str,
    project_root: &PathBuf,
    model: Option<String>,
    agent: Option<String>,
    wait: bool,
    show_diff: bool,
    json: bool,
) -> Result<()> {
    let root = project_root.to_string_lossy().into_owned();
    let response = client
        .request(Request::CreateTask {
            objective: objective.to_string(),
            project_root: root,
            model,
            agent_id: agent,
            mode: Default::default(),
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
        let Response::Events { events, last_seq } = response else {
            bail!("unexpected response fetching events");
        };
        for event in &events {
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
        if events.is_empty() || events.len() as i64 == last_seq - after + (last_seq - after) {
            tokio::time::sleep(Duration::from_millis(200)).await;
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
                    println!(
                        "{:<22} {:<12} {:<10} {}",
                        "ID", "STATUS", "PROJECT", "OBJECTIVE"
                    );
                    for task in tasks {
                        println!("{}", render::render_task_line(&task));
                    }
                }
            }
            Ok(())
        }
        TaskCommand::Show { task_id } => {
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
                for event in &events {
                    let line = render::render_event(event);
                    if !line.trim().is_empty() {
                        println!("{line}");
                    }
                }
                print!("{}", render::render_report(&task, None));
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
