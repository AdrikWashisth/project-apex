//! APEX command-line interface.
//!
//! The CLI is a *client* of the persistent runtime. It never re-implements
//! agent logic: it connects over the versioned protocol, streams events and
//! renders them. Tasks therefore survive CLI disconnection.

mod commands;
mod daemon;
mod render;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "apex",
    version,
    about = "APEX — an autonomous software engineering platform",
    long_about = "APEX runs AI agents against real repositories, verifies their work with \
observable evidence, and keeps tasks alive independently of this terminal."
)]
pub(crate) struct Cli {
    /// Directory to run in (defaults to the current directory).
    #[arg(long, short = 'C', global = true)]
    pub cwd: Option<String>,

    /// Emit machine-readable JSON instead of formatted output.
    #[arg(long, global = true)]
    pub json: bool,

    /// Increase log verbosity (repeatable).
    #[arg(long, short = 'v', global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Start (or attach to) the persistent runtime daemon.
    Runtime {
        /// Run in the foreground instead of detaching.
        #[arg(long)]
        foreground: bool,
        /// Shut down a running runtime and exit.
        #[arg(long)]
        stop: bool,
    },
    /// Start a task from a natural-language objective. Progress is streamed
    /// and the command waits for the task to reach a terminal state.
    Run {
        /// The task objective.
        objective: String,
        /// Model reference in provider/model form.
        #[arg(long)]
        model: Option<String>,
        /// Agent id to use.
        #[arg(long)]
        agent: Option<String>,
        /// Run several agents on the task, comma-separated
        /// (e.g. --agents apex-debugger,apex-reviewer).
        #[arg(long)]
        agents: Option<String>,
        /// Let team members run concurrently instead of handing work forward.
        #[arg(long)]
        parallel: bool,
        /// Execute a workflow definition file.
        #[arg(long, value_name = "FILE")]
        workflow: Option<String>,
        /// Print the resulting Git diff when finished.
        #[arg(long)]
        diff: bool,
        /// Return as soon as the task is created instead of waiting for it.
        #[arg(long)]
        no_wait: bool,
    },
    /// Task management.
    #[command(subcommand)]
    Task(TaskCommand),
    /// Agent management.
    #[command(subcommand)]
    Agents(AgentsCommand),
    /// Model provider management.
    #[command(subcommand)]
    Models(ModelsCommand),
    /// Show runtime and task status.
    Status,
    /// Show the current diff for a task or the current project.
    Diff {
        /// Task id (defaults to the most recent task for this directory).
        task_id: Option<String>,
    },
    /// Run verification checks for a task.
    Verify {
        /// Task id (defaults to the most recent task for this directory).
        task_id: Option<String>,
    },
    /// Show the effective configuration.
    Config,
    /// Print environment and installation diagnostics.
    Doctor,
}

#[derive(Subcommand)]
pub(crate) enum TaskCommand {
    /// List recent tasks.
    List {
        /// Maximum number of tasks to show.
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// Show a task and its events.
    Show {
        task_id: String,
        /// Also list the subtasks of a multi-agent task.
        #[arg(long)]
        subtasks: bool,
    },
    /// Show the execution plan and scheduling waves of a multi-agent task.
    Plan { task_id: String },
    /// List the subtasks of a multi-agent task.
    Subtasks { task_id: String },
    /// Resume an interrupted or failed task.
    Resume { task_id: String },
    /// Send an additional instruction to a finished task.
    Send {
        task_id: String,
        instruction: String,
    },
    /// Cancel a running task.
    Cancel { task_id: String },
}

#[derive(Subcommand)]
pub(crate) enum AgentsCommand {
    /// List available agents.
    List,
    /// Show a workflow definition file without running it.
    Plan {
        /// Workflow TOML file.
        workflow: String,
    },
    /// Run a task with a specific agent.
    Run {
        agent_id: String,
        /// The task objective.
        objective: String,
        /// Model reference in provider/model form.
        #[arg(long)]
        model: Option<String>,
        /// Return as soon as the task is created instead of waiting for it.
        #[arg(long)]
        no_wait: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum ModelsCommand {
    /// List configured providers and models.
    List,
    /// Set the default model.
    Set { reference: String },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let level = match cli.verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level)),
        )
        .with_writer(std::io::stderr)
        .with_target(false)
        .try_init();

    commands::dispatch(cli).await
}
