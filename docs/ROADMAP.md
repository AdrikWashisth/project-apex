# APEX Roadmap

This roadmap reflects what is actually implemented. A milestone is only marked
complete when the code exists, compiles, is tested and works.

## Status

| Milestone | Status | Notes |
| --- | --- | --- |
| 0 — Repository assessment | **Done** | Empty repo; Rust toolchain established; architecture and roadmap recorded |
| 1 — Runnable CLI foundation | **Done** | Workspace, config, doctor, one provider adapter, filesystem + shell tools |
| 2 — Agent runtime | **Done** | Typed manifests, tool calling, structured events, SQLite persistence, budgets |
| 3 — Verification | **Done** | Outcome contracts, real build/test/diff checks, bounded repair loop |
| 4 — Persistent local runtime | **Done** | Session manager, versioned IPC, task lifetime independent of client, reconnection |
| 5 — Multi-agent execution | **Done** | Plans, dependency scheduling, conflict avoidance, teams, workflows, handoff |
| 6 — IDE extension | **Partial** | Extension written and type-checked; never launched in VS Code |
| 7 — Agent creation and registry | **Partial** | Manifest format and validation exist; installation and registry do not |
| 8 — Memory and Kaizen | **Partial** | Scoped notes and lesson capture exist; evaluation and promotion do not |
| 9 — Production readiness | **Not started** | See the hardening list below |

## Completed

### Milestone 0 — Repository assessment

- The repository was empty; this is a greenfield build.
- Rust 1.99 installed via rustup (GNU toolchain host on this machine).
- Toolchain and dependencies pinned in `rust-toolchain.toml` and `Cargo.toml`.

### Milestone 1 — Runnable CLI foundation

- Modular workspace with nine crates and one binary.
- Layered configuration: defaults → user config → project config → env overrides.
- `apex doctor`, `apex config`, `apex models list/set`, `apex status`.
- OpenAI-compatible provider adapter with bounded retries, plus a deterministic
  offline provider.
- Filesystem tools (`read_file`, `write_file`, `edit_file`, `list_dir`), search
  (`glob_files`, `grep`), execution (`run_command`) and project inspection
  (`project_info`, `run_build`, `run_tests`).
- `run_tests` and `run_build` execute the project's real build commands and
  return real compiler output.

### Milestone 2 — Agent runtime

- Versioned TOML agent manifests with structural validation.
- Built-in agents: `apex-default`, `apex-reviewer`, `apex-debugger`.
- Bounded tool-calling loop with per-task budgets (steps, tool calls, tokens,
  cost, wall clock) and cancellation tokens.
- Structured, persisted event log with per-task monotonic sequence numbers.

### Milestone 3 — Verification

- `OutcomeContract` derived from the objective and the detected project type.
- Independent checks: real build, real tests, expected artefacts, real Git diff.
- Bounded diagnose-and-repair loop driven by failed check output.
- Evidence-based final report; verification status is emitted as an event and
  stored on the task.

### Milestone 4 — Persistent local runtime

- Runtime binds a local listener and publishes a connection-info file.
- Versioned, newline-delimited JSON protocol with request/response/event frames.
- Request ids are correlated on every response, including the handshake.
- CLI is a pure client: no agent logic, no model calls, no separate history.
- Tasks survive client disconnection; clients reconnect and replay events by
  sequence number.
- Runtime restart marks in-flight tasks as interrupted instead of losing them.
- Shutdown signals cancellation to running executors before dropping handles.
- Approval board for gated actions, with deny-on-timeout.

### Milestone 5 — Multi-agent execution

- `apex-orchestrator` crate: plan validation (DAG, cycles, dangling deps),
  dependency-ordered scheduling into **waves**, and conflict-aware wave packing.
- Manual multi-agent mode: `apex run --agents a,b,c`, sequential (handoff) or
  `--parallel`.
- Defined workflows: `apex run --workflow file.toml`, with `apex agents plan`
  to preview one.
- Read-only agents derived from their manifest run concurrently; agents that can
  modify the workspace are serialised unless their write scopes are provably
  disjoint.
- A workflow cannot grant an agent more power than its own manifest allows.
- Agent-to-agent handoff: a step receives the actual findings of its
  dependencies as context.
- Shared budget: a fan-out cannot multiply the parent task's spend.
- Every plan step becomes a persisted subtask with its own status, result and
  event stream.
- `apex task plan` and `apex task subtasks` to inspect execution.

### Testing

The suite runs with `cargo test --workspace` and gates on clippy `-D warnings`.
| Suite | Covers |
| --- | --- |
| Unit tests | config, manifests, protocol encoding, storage, tools |
| `e2e_vertical_slice` | scripted provider makes real file changes in a real Git repo, verified by a real `rustc` invocation; permission denial |
| `runtime_ipc` | real socket: handshake, bad-token rejection, task creation, event streaming, reconnect replay |
| `runtime_lifecycle` | cancellation, shutdown ordering, crash recovery, mode rejection |
| `security_regression` | path traversal, `.git` protection, permission profiles, no-shell |
| `multi_agent` | teams run and record subtasks, workflows execute, conflict serialisation, shared budget |
| `shipped_manifests` | the manifests in `agents/` are parsed by the real validator |

### Partial: memory and lessons

- Scoped notes (project / agent / user) persisted in SQLite.
- Verification repair outcomes recorded as project lessons.

## Next: Milestone 6 — IDE extension (frontend written, unverified)

- `extensions/vscode/` implements the frontend in TypeScript:
  - `client.ts` — wire protocol with request correlation and event fan-out
  - `runtime.ts` — discovery, handshake, protocol-version check, autostart
  - `tree.ts` — tasks and plan/subtasks grouped by scheduling wave
  - `approval.ts` — gated actions as modal prompts
  - `diff.ts`, `extension.ts` — editor diff view and wiring
- It contains no agent logic; every action is a protocol message.
- **Honest status:** the extension passes `tsc --noEmit` and compiles to
  JavaScript, but it has never been launched inside VS Code on this machine.
  Treat it as unverified until someone runs it in the editor.

Remaining for this milestone:
- Launch it in VS Code and confirm the views render.
- Bidirectional continuity test: start a task in the IDE, resume it from the
  CLI, confirm exactly one task.
- Live event streaming into the tree views (currently polled).

### Also delivered alongside Milestone 6

- **`apex tui`** — a ratatui terminal interface over the same runtime.
- **One-shot installer** — `install.sh` and `install.ps1`.
- **Honest parallelism** — `--parallel` refuses unsafe teams instead of
  silently serialising them.

## Next: Milestone 7 — Agent registry

- `apex agents install/remove/create`, local registry with version pinning.
- Provenance and signature verification for published manifests.
- Explicit-permission execution for agents that carry executable extensions.

## Next: Milestone 8 — Memory and Kaizen

- Baseline evaluation suites per agent.
- Metrics: task success rate, verification pass rate, repair attempts, cost,
  tool failure rate.
- Versioned improvement proposals evaluated against the baseline before
  promotion, with rollback.

## Next: Milestone 9 — Production readiness

- Named pipes on Windows (closing the loopback-TCP gap).
- OS-level sandboxing, honestly documented for each platform.
- Tamper-evident audit log.
- Installers, CI matrix across platforms, release signing.

## Explicit non-goals for now

Per the "modular monolith first" constraint, APEX will **not** introduce
Kubernetes, a message bus, microservices, a plugin hot-loading system or a
public marketplace until real scale and real demand justify them..