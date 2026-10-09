# APEX — Architecture

APEX is a model-agnostic, multi-agent software engineering platform. This
document describes how the system is built today, in the code.

## Core principle

**An agent's claim of success is not proof of success.** APEX treats generated
code as a hypothesis and verifies it with observable evidence: compiler exit
codes, test results, Git diffs, file existence checks and reproducible command
output. A model's self-reported confidence is never accepted as verification.

## Crate layout

```text
crates/
  apex-core/           configuration, project discovery, Git, process, path safety
  apex-protocol/       wire protocol, task/event model, message model
  apex-models/         provider abstraction + OpenAI-compatible adapter + fake
  apex-tools/          typed, permissioned tools
  apex-memory/         SQLite persistence for tasks, events and notes
  apex-agent/          agent manifests, catalog, bounded execution loop
  apex-verification/   outcome contracts, independent checks, repair guidance
  apex-runtime/        session manager, task executor, IPC server and client
  apex-cli/            the `apex` binary (a client of the runtime)
```

Dependencies flow strictly downwards. `apex-core` depends on nothing internal.
`apex-protocol` depends only on `apex-core`. Everything else builds on those.

## Layering

```text
                  ┌───────────────┐
     CLI  ──────►│               │
                  │    Runtime    │  task lifecycle, scheduling, budgets
   IDE  ──────►   │   (server)    │  persistence, event fan-out
                  │               │
                  └───────┬───────┘
                          │
          ┌───────────────┼────────────────┐
          │               │                │
      ┌────▼────┐   ┌──────▼──────┐   ┌─────▼──────┐
      │  Agent  │   │Verification │   │   Memory   │
      │  loop   │   │  checks     │   │  (SQLite)  │
      └────┬────┘   └──────┬──────┘   └────────────┘
           │               │
       ┌───▼───────────────▼───┐
       │       Tools           │  path-safe, permissioned side effects
       └───────────┬───────────┘
                   │
       ┌───────────▼───────────┐
       │  Model provider(s)    │  one interface, many adapters
       └───────────────────────┘
```

## One runtime, many clients

The CLI and the IDE extension are both thin clients. Neither contains an agent
loop, a task history or a model-call path of its own. They speak a versioned,
newline-delimited JSON protocol to a persistent runtime:

```text
apex CLI ─┐
          ├─► local IPC ─► Runtime ─► Agents / Tools / Memory ─► Model Router
VS Code ──┘                             │
                                        └─► SQLite (tasks, events, notes)
```

Consequences that are enforced by design:

- A task continues running when a terminal closes. Client disconnection is not
  cancellation.
- Any client can reconnect and reconstruct the current session by reading
  persisted events from a sequence number.
- The CLI and the IDE see the same task id, the same events and the same diff.

### Transport

On Unix the runtime binds a Unix-domain socket. On other platforms it binds a
loopback TCP listener on `127.0.0.1` and writes a connection-info file
(`~/.apex/state/runtime.json`) containing the endpoint and a random bearer
token that clients must present during the handshake. This is honest about
being weaker than a named pipe: any local process that can read the user's home
directory can reach the socket. See SECURITY_MODEL.md.

Named pipes on Windows are the intended next step and are not yet implemented;
the runtime logs a warning and falls back to loopback TCP.

### Protocol

`apex-protocol` defines three frame types tagged by a `frame` field:

- `request` — a client operation with a correlation id
- `response` — the correlated reply
- `event` — an unsolicited task event pushed to subscribed clients

Requests are tagged by an `op` field. The handshake is mandatory and validates
both the protocol version and the connection token. Every response echoes the
request id, including the handshake response, so a client can always correlate.

Large payloads (`Task`, task lists, event batches, diffs) are boxed inside
`Response` so a `Frame` stays small regardless of what it carries.

## Task lifecycle

1. **Create** — the objective, project root, model and agent are recorded and
   the task starts as `Pending`.
2. **Run** — the status becomes `Running` and the agent loop executes.
3. **Verify** — independent checks run: build, tests, expected artefacts and a
   real Git diff.
4. **Repair** — if verification fails and the repair budget allows, the failure
   report is fed back to the agent as a new instruction and the task is
   re-verified. The loop is bounded by `budget.max_repair_attempts`.
5. **Finish** — the task becomes `Completed`, `Failed` or `Cancelled`, and a
   report containing the actual check results is emitted.

States: `Pending → Running → Verifying → Completed | Failed | Cancelled`,
plus `WaitingApproval` for gated actions.

### Recovery

On startup the runtime marks any task left in `Running` by a previous process as
`Failed` with an explanatory error, rather than silently resurrecting it. The
user can then `apex task resume <id>`, which rebuilds the conversation from the
persisted event log and continues.

## Agent execution loop

The loop is deliberately boring and bounded:

```text
loop {
  check cancellation, step budget, wall clock, token budget, cost budget
  call the model with the conversation and the permitted tool schemas
  accumulate usage and emit a Usage event
  emit the assistant message
  if the model requested no tools → finish with its text
  for each tool call {
    check the tool-call budget
    emit ToolStarted
    resolve permission and, if gated, request approval
    execute the tool
    emit ToolFinished
    append the tool result to the conversation
  }
}
```

Every exit path is either a model decision, a budget, cancellation or an error.
There is no unbounded retry loop.

## Model abstraction

All model access goes through the `ModelProvider` trait. An adapter reports its
own `ModelCapabilities` (tools, streaming, context window, output limit) so the
runtime can detect and explicitly reject unsupported features rather than
assuming every model behaves the same way.

The OpenAI-compatible adapter implements `/chat/completions` with bounded
retries and backoff on `429` and `5xx`. It works against the OpenAI API and
against any server exposing the same contract, including local model servers.
The `Fake` provider is deterministic and offline; it exists for tests and for
running the CLI without credentials.

Cost is estimated from a small built-in price table when a provider does not
report exact costs, and is labelled as an estimate wherever it surfaces.

## Tools

Tools are typed: each declares an id, a JSON schema, a risk class and a
structured result. Results are returned to the model as text *and* as structured
data, so the runtime and clients never have to parse terminal noise.

Initial tools: `read_file`, `write_file`, `edit_file`, `list_dir`,
`glob_files`, `grep`, `run_command`, `git_status`, `git_diff`, `project_info`,
`run_build`, `run_tests`.

Two invariants are enforced centrally in `ToolContext`:

- **Path containment.** Every path is resolved and canonicalised against the
  workspace root before use. `..`, absolute paths and symlinks that escape the
  root are rejected. Writes inside `.git` are refused outright.
- **Permission profile.** The active profile decides what may run at all, and
  which risk classes require explicit approval before execution.

## Verification

`OutcomeContract` states the objective, constraints, acceptance criteria,
expected artefacts and verification commands. `verify` runs the checks and
returns a `VerificationReport` of `CheckResult`s. `repair_instruction` turns
failed checks into a concrete, evidence-bearing instruction for the agent.

Verification is intentionally outside the agent: it uses the same tool
implementation but a separate code path, so a model cannot mark its own work as
verified.

## Memory

SQLite holds four things: tasks, the event log, scoped memory notes and (via the
store API) evaluation history. Events are assigned a monotonic per-task
sequence number, which is what makes reconnect-and-resume exact: a client asks
for events after sequence *n* and gets precisely what it missed.

Memory retrieval is scoped by project and agent so that an agent working in one
project does not receive another project's context.

## Extension points

- **New model provider** — implement `ModelProvider`, add a `ProviderKind`, and
  register it in `apex_models::build_provider`.
- **New tool** — implement `Tool`, register it in `ToolRegistry::default_set`,
  and declare its risk class and schema.
- **New agent** — write a TOML manifest; validation runs before the agent is
  ever loaded.