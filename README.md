# APEX

**An autonomous software engineering platform that verifies its own work.**

APEX runs AI agents against real repositories. It is not a chatbot that prints
code — it is an execution system that inspects a real development environment,
takes authorised actions, evaluates the results, and reports whether the
engineering objective was actually achieved.

> **An agent's claim of success is not proof of success.**
> APEX verifies results with observable evidence: compiler exit codes, test
> results, Git diffs, file-existence checks and reproducible command output.

## What works today

- **Real model access** — an OpenAI-compatible provider adapter, so APEX works
  against OpenAI and against any local server exposing the same API.
- **Real tools** — read/write/edit files, list, glob, grep, run commands, inspect
  and diff Git, detect the project, run its actual build and test commands.
- **Bounded execution** — every task has hard limits on steps, tool calls,
  tokens, cost and wall-clock time, plus cancellation.
- **Independent verification** — after the agent finishes, APEX runs its own
  checks and only reports success if those checks pass.
- **Bounded self-repair** — if verification fails, the failure evidence is fed
  back to the agent for a bounded number of repair attempts.
- **A persistent runtime** — tasks survive terminal closure. The CLI and a
  future IDE extension talk to the same runtime over a versioned protocol.
- **Multi-agent execution** — hand a task to a team or a workflow. APEX builds a
  plan, schedules it into dependency-ordered waves, and refuses to run two
  agents concurrently if they could write the same file.
- **Security boundaries** — every filesystem path is confined to the workspace;
  permission profiles gate what an agent may do; commands never go through a
  shell.

## Install (build your own)

Requires Rust 1.99 or newer.

```bash
git clone <repository-url> apex
cd apex
cargo build --release
# the binary is at target/release/apex (apex.exe on Windows)
```

## Install (oneshot command)

One command. The installer puts Rust, the binary and your PATH in order, and
leaves your configuration, agents and task history alone if you re-run it.

**Linux / macOS**

```bash
curl -fsSL https://raw.githubusercontent.com/AdrikWashisth/project-apex/main/install.sh | bash
```

**Windows** (PowerShell)

```powershell
iwr -useb https://raw.githubusercontent.com/AdrikWashisth/project-apex/main/install.ps1 | iex
```

Or clone and run the script for your platform:

```bash
git clone https://github.com/AdrikWashisth/project-apex.git
cd project-apex
./install.sh          # Linux / macOS
# .\install.ps1        # Windows
```

If you already have Rust 1.99 or newer, you can skip all of that:

```bash
cargo build --release --bin apex
# binary at target/release/apex (apex.exe on Windows)
```

Then check the environment and configure a model:

```bash
apex doctor
apex models set openai/gpt-4o-mini   # then: export OPENAI_API_KEY=sk-...
```

## Quick start

Check the environment:

```bash
apex doctor
```

Configure a model. APEX ships an dummy offline provider so you can try everything
before you have an API key:

```toml
# ~/.apex/config/config.toml
default_model = "local/offline"

[providers.local]
kind = "fake"
base_url = "http://localhost"
models = ["offline"]
```

Or point it at a real provider:

```bash
export OPENAI_API_KEY=sk-...
apex models set openai/gpt-4o-mini
```

Then run a task against a real repository:

```bash
cd ~/code/my-project
apex run "Add input validation to the user creation endpoint"
```

APEX starts a task, streams progress, runs the project's real tests, repairs
failures if it can, and finishes with an evidence-based report:

```text
task task_9f2c1a running (apex-default using openai/gpt-4o-mini)
  ⚙ read_file src/users.rs …
  ✓ read_file: read src/users.rs (214 lines)
  ⚙ edit_file {"path":"src/users.rs", …}
  ✓ edit_file: edited src/users.rs (1 replacement(s))
  ⚙ run_tests …
  ✓ run_tests: `cargo test` passed
  ✓ verification: 3/3 checks passed

=== Task task_9f2c1a: COMPLETED ===

Added validation to the user creation endpoint and wired it into the handler.

Verification:
  [PASS] build
  [PASS] tests
  [PASS] diff

Evidence: 7 tool call(s), 9 step(s), 0 repair attempt(s), 24180 tokens
```

## Commands

```text
apex run <objective>        Start a task and stream it to completion
apex task list              List recent tasks
apex task show <id>         Show a task and its event stream
apex task plan <id>         Show the plan and scheduling waves
apex task subtasks <id>     List the subtasks of a multi-agent task
apex task resume <id>       Resume an interrupted or failed task
apex task send <id> <text>  Continue a finished conversation
apex task cancel <id>       Cancel a running task
apex agents list            List available agents
apex agents plan <file>     Preview a workflow without running it
apex agents run <id> <goal> Run a task with a specific agent
apex models list            List providers and models
apex models set <p/m>       Set the default model
apex status                 Runtime and task status
apex diff                   Show the current Git diff
apex verify                 Run verification checks
apex config                 Show the effective configuration
apex doctor                 Environment diagnostics
apex runtime                Start or attach to the persistent runtime
```

Add `--json` to any command for machine-readable output.

## Interfaces

APEX has three frontends, all talking to the same runtime:

| Interface | Command | Use it for |
| --- | --- | --- |
| CLI | `apex run "..."` | Scripts, automation, `--json` output |
| TUI | `apex tui` | Watching a run live, keyboard-driven |
| VS Code | see `extensions/vscode/` | Editor-integrated diffs and approvals |

### The terminal interface

```bash
apex tui
```

A live view of the runtime: tasks on the left, the selected task's event stream
in the centre, and its plan and subtasks on the right. It polls the runtime,
so it stays correct even if the task was started from the CLI or another client.

| Key | Action |
| --- | --- |
| `↑` / `↓` | Move through tasks, or scroll the focused pane |
| `Tab` | Switch focus between Tasks, Events and Plan |
| `n` | New task |
| `a` | New task with an agent team (`--agents a,b`) |
| `w` | Run a workflow (`--workflow file.toml`) |
| `c` | Cancel the selected task |
| `d` | Show the selected task's diff |
| `v` | Run verification on the selected task |
| `r` | Refresh now |
| `?` | Show the key help |
| `q` / `Ctrl-C` | Quit |

The TUI is a frontend. It holds no task state of its own — close it and the
task keeps running, and reopening it shows the same task from the runtime.

## Running multiple agents

By default a task runs on one agent. You can hand it to a team, or run a
workflow:

```bash
# A team. Members run in order, each receiving the previous one's findings.
apex run "Harden the input validation" --agents apex-debugger,apex-reviewer

# Or let them work concurrently (safe: APEX will not run two agents that could
# write the same file at once).
apex run "Audit the codebase" --agents apex-reviewer,apex-reviewer --parallel

# A workflow: review first, then fix what the review found.
apex run "Improve reliability" --workflow workflows/review-then-fix.toml
```

APEX builds an execution plan, schedules it into waves, and refuses to run two
agents concurrently if they could both write the same file:

```text
$ apex run "Improve reliability" --workflow workflows/review-then-fix.toml

wave 0: review (apex-reviewer)
wave 1: fix (apex-debugger)
```

Read-only agents (like `apex-reviewer`) never conflict, so they always run in
parallel. Inspect any run afterwards:

```bash
apex task plan <task-id>       # steps and waves
apex task subtasks <task-id>   # per-agent status and result
```

The whole task still finishes with a single independent verification pass over
the workspace — a team is not trusted just because several agents agreed.

## Architecture at a glance

```text
        apex CLI ─┐
                   ├─ local IPC ─► persistent runtime ─► agents / tools / memory
    VS Code ──────┘                                            │
                                                                ▼
                                                       model providers
```

The CLI and IDE are both thin clients of one runtime. Neither contains an agent
loop or a model-call path of its own, so a task started in one is the same task
you can watch, approve or cancel from the other.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the full design.

## Verification model

APEX does not equate generated code with completed work. After the agent
finishes, APEX independently checks:

1. The project **builds** (real compiler output, real exit code).
2. The project's **tests pass** (real test runner output).
3. **Expected artefacts** exist, if the contract declares any.
4. A **real Git diff** exists and is non-empty.

If a check fails, the failure output becomes the agent's next instruction, up to
a bounded number of repair attempts. If it still fails, the task is reported as
failed — with the reason — rather than quietly succeeding.

## Security

APEX treats repository content, model output and agent manifests as untrusted.

- Every path is resolved and confined to the workspace; `..`, absolute paths and
  symlink escapes are rejected; writes into `.git` are refused.
- Permission profiles (`read_only`, `assisted`, `controlled_autonomous`,
  `advanced`) gate what may run.
- Commands run with an explicit argument vector — **no shell** — so pipes,
  redirection and shell injection are unavailable.
- API keys come from environment variables only. They are never written to
  config or logs.

There is deliberately no "unrestricted" mode that disables these checks.

Read [docs/SECURITY_MODEL.md](docs/SECURITY_MODEL.md) for the honest limitations,
including the fact that APEX does not sandbox agent processes at the OS level.

## Documentation

| Document | Contents |
| --- | --- |
| [ARCHITECTURE.md](docs/ARCHITECTURE.md) | Crate layout, layering, task lifecycle, protocol |
| [ROADMAP.md](docs/ROADMAP.md) | What is implemented, what is next |
| [SECURITY_MODEL.md](docs/SECURITY_MODEL.md) | Controls and their limits |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Ground rules and language policy |

## Status

This is an early, actively developed project. Milestones 1–5 (CLI, agent runtime,
verification, persistent runtime, multi-agent execution) are implemented and
tested. The agent registry and controlled self-improvement are on the roadmap
and are **not** yet available — the roadmap marks exactly where the boundary is.

The VS Code extension is **partially** done, and the split matters:

- Its protocol client is verified against a real running runtime. A test starts
  an actual APEX runtime and drives the compiled extension client through the
  handshake, task/agent/status requests and interleaved request correlation.
  This caught two bugs that type-checking could not: the client never sent its
  `Hello` frame, and it had no handshake timeout.
- **It has never been launched inside VS Code.** The views, approval modals and
  diff view are unexercised. Do not assume the UI works.

Model-driven `orchestrated` mode is accepted and validated by the protocol, but
no planner ships yet: supplying a plan works, omitting one fails with a clear
error.

### Verifying the extension yourself

```bash
apex runtime                  # in another terminal — starts the daemon
cd extensions/vscode
npm install
npx tsc -p ./                 # compiles the extension to out/

# Drive the compiled client against the live runtime:
node scripts/probe.js $(jq -r .endpoint ~/.apex/state/runtime.json) \
                    $(jq -r .token    ~/.apex/state/runtime.json)
```

It prints one line per check and exits non-zero on the first failure.

`cargo test --workspace` runs that probe for you when the extension has been
compiled, and skips with an explanatory message when it has not.

## Development

```bash
cargo test --workspace          # unit + integration suites
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

The test suite deliberately uses a deterministic offline provider for the model,
but every filesystem write, Git call and compiler invocation is real. The
end-to-end test compiles the file it creates with an actual `rustc`.

## License

GPL-3.0-only. See [LICENSE](LICENSE).
