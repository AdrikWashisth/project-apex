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
- **Security boundaries** — every filesystem path is confined to the workspace;
  permission profiles gate what an agent may do; commands never go through a
  shell.

## Install

Requires Rust 1.99 or newer.

```bash
git clone <repository-url> apex
cd apex
cargo build --release
# the binary is at target/release/apex (apex.exe on Windows)
```

## Quick start

Check the environment:

```bash
apex doctor
```

Configure a model. APEX ships an offline provider so you can try everything
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
apex task resume <id>       Resume an interrupted or failed task
apex task send <id> <text>  Continue a finished conversation
apex task cancel <id>       Cancel a running task
apex agents list            List available agents
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

This is an early, actively developed project. Milestones 1–4 (CLI, agent runtime,
verification, persistent runtime) are implemented and tested. Multi-agent
orchestration, the IDE extension and the agent registry are on the roadmap and
are **not** yet available — the roadmap marks exactly where the boundary is.

## License

Apache-2.0. See [LICENSE](LICENSE).