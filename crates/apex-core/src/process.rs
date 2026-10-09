//! Bounded subprocess execution shared by Git helpers and tools.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::error::{ApexError, Result};

/// Default cap on captured output per stream (1 MiB).
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 1024 * 1024;

/// Options controlling a subprocess run.
#[derive(Debug, Clone)]
pub struct RunOptions {
    /// Working directory for the process.
    pub cwd: PathBuf,
    /// Maximum wall-clock duration before the process is killed.
    pub timeout: Duration,
    /// Additional environment variables.
    pub env: Vec<(String, String)>,
    /// Optional stdin payload.
    pub stdin: Option<String>,
    /// Maximum bytes captured from each of stdout and stderr.
    pub max_output_bytes: usize,
}

impl RunOptions {
    /// Create options with the given working directory and default timeout.
    pub fn in_dir(cwd: impl Into<PathBuf>) -> Self {
        RunOptions {
            cwd: cwd.into(),
            timeout: Duration::from_secs(120),
            env: Vec::new(),
            stdin: None,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        }
    }
}

/// The structured result of a subprocess run.
#[derive(Debug, Clone)]
pub struct CommandOutput {
    /// Process exit code, if the process exited normally.
    pub exit_code: Option<i32>,
    /// Whether the process exited with status zero.
    pub success: bool,
    /// Captured standard output.
    pub stdout: String,
    /// Captured standard error.
    pub stderr: String,
    /// Whether the process was killed after exceeding its timeout.
    pub timed_out: bool,
    /// Whether either stream exceeded the capture limit.
    pub truncated: bool,
}

impl CommandOutput {
    fn from_parts(
        exit_code: Option<i32>,
        timed_out: bool,
        stdout: String,
        stderr: String,
        truncated: bool,
    ) -> Self {
        CommandOutput {
            success: !timed_out && exit_code == Some(0),
            exit_code,
            stdout,
            stderr,
            timed_out,
            truncated,
        }
    }
}

/// Run a program with the given arguments under a bounded timeout.
pub async fn run(program: &str, args: &[String], options: &RunOptions) -> Result<CommandOutput> {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(&options.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (key, value) in &options.env {
        command.env(key, value);
    }

    let mut child = command.spawn().map_err(|e| ApexError::Tool {
        tool: program.to_string(),
        message: format!("failed to spawn: {e}"),
    })?;

    if let Some(input) = &options.stdin {
        if let Some(mut stdin) = child.stdin.take() {
            let bytes = input.clone().into_bytes();
            tokio::spawn(async move {
                let _ = stdin.write_all(&bytes).await;
                let _ = stdin.flush().await;
                // Dropping stdin closes the pipe.
            });
        }
    } else {
        drop(child.stdin.take());
    }

    let limit = options.max_output_bytes;
    let output_fut = async {
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let stdout_task = read_capped(stdout, limit);
        let stderr_task = read_capped(stderr, limit);
        let (out, err) = tokio::join!(stdout_task, stderr_task);
        let status = child.wait().await;
        (out, err, status)
    };

    match tokio::time::timeout(options.timeout, output_fut).await {
        Ok((out, err, status)) => {
            let (stdout, out_trunc) = out?;
            let (stderr, err_trunc) = err?;
            let status = status?;
            Ok(CommandOutput::from_parts(
                status.code(),
                false,
                stdout,
                stderr,
                out_trunc || err_trunc,
            ))
        }
        Err(_) => {
            // The future was dropped, which triggers `kill_on_drop`.
            Ok(CommandOutput::from_parts(
                None,
                true,
                String::new(),
                format!(
                    "process '{program}' exceeded the {}s timeout",
                    options.timeout.as_secs()
                ),
                false,
            ))
        }
    }
}

async fn read_capped<R>(reader: Option<R>, limit: usize) -> Result<(String, bool)>
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;
    let mut reader = match reader {
        Some(r) => r,
        None => return Ok((String::new(), false)),
    };
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let mut truncated = false;
    loop {
        let n = reader.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        if buf.len() >= limit {
            truncated = true;
            continue;
        }
        let remaining = limit - buf.len();
        let take = n.min(remaining);
        buf.extend_from_slice(&chunk[..take]);
        if take < n {
            truncated = true;
        }
    }
    Ok((String::from_utf8_lossy(&buf).into_owned(), truncated))
}
