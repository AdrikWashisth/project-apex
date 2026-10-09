# IDE Integration

The IDE extension is a **frontend**. It does not contain an agent loop, a task
history or a model-call path. It connects to the same persistent runtime the
CLI uses and speaks the same versioned protocol.

This is a deliberate architectural constraint, not an implementation detail:

> The CLI and IDE must not implement separate agent brains, task histories or
> model-call loops.

The consequence that matters to users: start a task in VS Code, close the
window, reopen it, and the task is still running under the same id. Resume it
from the terminal with `apex task resume <id>` and you are watching the same
conversation.

## Status

**The extension is not built yet.** Milestone 6 is on the roadmap. What exists
today is the half that matters most and is hardest to retrofit:

- A stable, versioned protocol (`apex-protocol`, `PROTOCOL_VERSION = 1`)
- A persistent runtime that outlives any client
- A discovery file at `~/.apex/state/runtime.json` with the endpoint and token
- Full event persistence with monotonic sequence numbers, so a client can
  reconnect and replay exactly what it missed
- Request correlation and live event push on the same connection

Those are the hard parts, and they are tested (`tests/tests/runtime_ipc.rs`).

## Connecting

```text
1. Locate the apex executable            (PATH, common install dirs, settings)
2. Read ~/.apex/state/runtime.json       (endpoint + bearer token)
3. Open a socket to `endpoint`
4. Send Request::Hello { protocol_version, client_name, client_version, token }
5. Check the returned protocol_version against the one you were built for
```

If the runtime is not running, the extension should offer to start it
(`apex runtime`) rather than starting agent logic in-process. If the protocol
versions disagree, show the mismatch and tell the user to upgrade — never
degrade silently into a partial mode.

## Protocol surface

| Request | Purpose | Response |
| --- | --- | --- |
| `Hello` | handshake + version check | `Hello` / `Error` |
| `CreateTask` | start work in the open workspace | `Task` |
| `ListTasks` | recent tasks | `TaskList` |
| `ShowTask` | task state | `Task` |
| `ListSubtasks` | subtasks of a multi-agent task | `SubtaskList` |
| `ShowPlan` | plan and scheduling waves | `Plan` |
| `SendInstruction` | continue a finished task | `Ok` |
| `TaskEvents` | replay + subscribe | `Events` |
| `ListAgents` | available agents | `Agents` |
| `CancelTask` | stop work | `Ok` |
| `ResumeTask` | continue interrupted work | `Ok` |
| `ResolveApproval` | approve or reject a gated action | `Ok` |
| `GetDiff` | the real Git diff | `Diff` |
| `Verify` | run independent checks | `Verification` |
| `Status` | runtime state | `Status` |
| `Shutdown` | stop the runtime | `Ok` |

Frames are newline-delimited JSON tagged by a `frame` field
(`request` / `response` / `event`). Requests carry a correlation `id`; the
runtime echoes it on the response. The handshake response is correlated too —
get this wrong and the client will hang waiting for a response that arrives
under a different id.

## Reconnection

This is the pattern the extension must implement:

```text
subscribe to live events      (a long-lived TaskEvents subscription)
track last_seq locally
on any event with seq > last_seq → update the UI, last_seq = seq
on reconnect:
    request TaskEvents { after_seq: last_seq }
    replay anything the server returns (dedupe by seq, since events may
    overlap the subscribe window)
    resume live updates
```

Deduplication by sequence number is required: an event can arrive between the
history request and the subscription taking effect.

## What the extension should render

- Conversation messages (assistant text, tool calls as collapsed entries)
- Tool activity with real results, not "thinking…" placeholders
- For multi-agent tasks: the plan and its waves, which agent is running which
  step, and each subtask's result as it lands
- Approval prompts with the action, its risk class and its arguments
- Verification results — pass/fail per check with the real command output
- The real Git diff
- Runtime and task status

## Approval UX

When the runtime emits `ApprovalRequested`, the user sees the action, the risk
class and the arguments, and responds with `ResolveApproval`. The runtime denies
by default if nobody responds before the timeout, so an unattended run fails
closed.

## TypeScript boundary

If the extension is written in TypeScript, that is fine — it is required by the
VS Code extension API. The constraint is that the extension must not reimplement
agent behaviour. It translates user actions into protocol messages and renders
responses. It must not:

- construct prompts and call a model directly
- maintain its own task or conversation store
- re-implement tool execution or permission checks

## Testing continuity

The acceptance test for continuity is: start a task through one interface, then
inspect and resume it through the other, and confirm there is exactly one task —
not a duplicate. The runtime makes this testable without the extension because
task identity lives in the runtime, not in any client.