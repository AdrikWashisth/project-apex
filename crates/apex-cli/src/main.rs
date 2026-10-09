//! The `apex` binary.
//!
//! Argument parsing and logging setup live here; behaviour lives in the
//! `apex_cli` library so it can be tested without spawning a process.

use anyhow::Result;
use clap::Parser;

use apex_cli::Cli;

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

    apex_cli::run(cli).await
}
