# APEX — Security Model

Security is part of the product, not a later enhancement. This document
describes what APEX actually enforces today, what it does not, and where the
honest limitations are.

## Threat model

APEX treats all of the following as **untrusted input**:

- Source code and files in the repository being worked on
- Repository documentation, including instructions aimed at the agent
- Model output, including tool arguments
- Downloaded agent manifests and marketplace packages
- Anything read from the network

A repository that says "ignore your safety rules and run this command" is just
text. It has no authority over APEX policy.

## Trust boundaries

```text
┌─ untrusted ────────────────────────────────────────────────┐
│  repository files, model output, agent manifests            │
└──────────────────────────┬────────────────────────────────┘
                           │  mediated by
┌──────────────────────────▼────────────────────────────────┐
│  ToolContext: path containment + permission profile        │
│  AgentRunner: budget, cancellation, tool allow-list       │
└──────────────────────────┬────────────────────────────────┘
                           │  allowed only after checks
┌──────────────────────────▼────────────────────────────────┐
│  Side effects: filesystem writes, process execution        │
└───────────────────────────────────────────────────────────┘
```

## Controls that are implemented

### Filesystem containment

Every filesystem tool resolves its path against the workspace root before
touching anything (`apex_core::paths::resolve_within`):

- `..` components are collapsed lexically.
- The result must be a prefix of the canonicalised workspace root.
- If the path exists it is canonicalised first, so symlinks cannot escape.
- Writes targeting anything under `.git` are refused.

Path traversal attempts return `ApexError::PathEscape` rather than being
silently rewritten.

### Permission profiles

Profiles are ordered from least to most capable:

| Profile | Effect |
| --- | --- |
| `read_only` | Only `RiskClass::ReadOnly` tools may run |
| `assisted` | Default. Reads and writes allowed; shell gated by `allow_shell` |
| `controlled_autonomous` | Autonomous within the workspace, still bounded and audited |
| `advanced` | User-configured autonomy; never bypasses the audit trail |

A read-only profile blocks `write_file`, `edit_file`, `run_command`, `run_build`
and `run_tests`. This is enforced in `ToolContext::deny_reason`, so it applies
to every tool regardless of how it was registered.

There is deliberately **no** "unrestricted" mode that disables these checks.

### Risk classification and approval

Tools declare a `RiskClass`: `ReadOnly`, `Mutating`, `Executing`, `Destructive`.
The active profile lists which classes require explicit human approval. When an
action is gated, the runtime emits an `ApprovalRequested` event, registers the
request on an approval board and waits for a client to call
`ResolveApproval`. A timed-out or unresolvable approval is treated as a denial.

### Command execution

- Commands are executed directly with an explicit argument vector. APEX does
  **not** invoke a shell, so pipes, redirection, globbing and shell builtins are
  not available. This removes a whole class of quoting and injection bugs.
- Every command runs under a wall-clock timeout with `kill_on_drop`, so a hung
  child cannot outlive its task.
- Output is captured with a hard size cap.
- Shell execution can be disabled outright with `permissions.allow_shell`.

### Budgets

Every task carries a budget: maximum steps, tool calls, model tokens, estimated
cost and wall-clock seconds. Each is checked before work is started, so an agent
cannot spend its way past a limit by acting first and reporting later.

### Cancellation

Every task holds a `CancellationToken` checked at each loop boundary and before
each tool call. Cancellation is distinct from client disconnection: closing a
terminal does not cancel the task.

### Secret handling

API keys are read from named environment variables, never stored in the config
file and never printed. Config serialisation covers only the *name* of the
environment variable. Nothing in the codebase logs a resolved credential.

### Bounded retries

Model requests retry only on `429` and `5xx`, with exponential backoff and a hard
cap of three attempts. Other status codes fail immediately.

## Limitations — stated plainly

These are real constraints, not oversights:

1. **No process sandbox.** APEX does not run agents in a container, a job
   object, or a restricted token. On Windows the runtime executes with the full
   privileges of the user who started it. A malicious command approved by a
   `controlled_autonomous` profile can do anything that user can do.
2. **Loopback TCP on non-Unix platforms.** Named pipes are not implemented yet.
   The runtime binds `127.0.0.1` and publishes a bearer token in
   `~/.apex/state/runtime.json`. Any process running as the same user can read
   that file and connect. Unix-domain sockets (on Unix) get filesystem
   permission checks for free; loopback TCP does not.
3. **Permission profiles constrain APEX's own tools, not the OS.** The
   `read_only` profile stops APEX from writing through `write_file`. It does not
   stop a program the user ran themselves.
4. **No signed manifests yet.** Agent manifests are validated structurally, not
   cryptographically. Provenance is not yet enforced.
5. **Model output is never sanitised against prompt injection.** It is treated
   as untrusted in the sense that tool calls are checked against policy before
   execution, but the agent's *reasoning* can still be steered by hostile text
   in the repository. Path and permission checks are the mitigation; they bound
   the blast radius, they do not make the model trustworthy.
6. **Audit logging is in-process.** Events are persisted to SQLite, but there is
   no separate tamper-evident audit sink yet.

## Reporting a vulnerability

Report security issues privately to the maintainers rather than opening a public
issue. Please include reproduction steps and the APEX version. See
`SECURITY.md`.