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
| 5 — Multi-agent execution | **Not started** | Single-agent mode only; orchestrator is a planned mode, not a dependency |
| 6 — IDE extension | **Not started** | Protocol is stable and ready for a VS Code client |
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
- CLI is a pure client: no agent logic, no model calls, no separate history.
- Tasks survive client disconnection; clients reconnect and replay events by
  sequence number.
- Runtime restart marks in-flight tasks as interrupted instead of losing them.
- Approval board for gated actions, with deny-on-timeout.

### Partial: memory and lessons

- Scoped notes (project / agent / user) persisted in SQLite.
- Verification repair outcomes recorded as project lessons.

## Next: Milestone 5 — Multi-agent execution

- Agent registration and a scheduler that can run several agents concurrently.
- Agent-to-agent messaging with explicit handoff contracts.
- Manual multi-agent mode (user assigns work to selected agents).
- Optional orchestrator as a *selectable mode*, not a mandatory intermediary.
- Conflict detection for concurrent edits to the same file.
- User-defined workflow sequences.

## Next: Milestone 6 — IDE extension

- VS Code extension that detects the CLI, checks protocol compatibility, and
  connects to the running runtime.
- Conversation, plan, tool-activity and diff views.
- Approval and cancellation controls.
- Bidirectional continuity: start in the IDE, resume in the CLI, same task id.

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
public marketplace until real scale and real demand justify them.