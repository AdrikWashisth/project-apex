# APEX VS Code extension

> **Status: not implemented.**
>
> This directory is a manifest skeleton, not a working extension. Nothing here
> has been built, tested or verified. It exists to pin down the intended
> command surface and to reserve the namespace.
>
> See [../../docs/IDE_INTEGRATION.md](../../docs/IDE_INTEGRATION.md) for the
> design and [../../docs/ROADMAP.md](../../docs/ROADMAP.md) (Milestone 6) for the
> plan.

## What is already done

The hard part of IDE integration is complete and tested in the Rust runtime:

- A versioned, newline-delimited JSON protocol (`apex-protocol`)
- A persistent runtime that outlives any client
- Discovery via `~/.apex/state/runtime.json`
- Full event persistence with monotonic sequence numbers, so a client can
  reconnect and replay exactly what it missed
- Request correlation and live event push

These are the pieces that are expensive to retrofit later, and they are covered
by `tests/tests/runtime_ipc.rs`.

## What remains

- `src/extension.ts` — activation, runtime discovery, handshake, version check
- `src/client.ts` — a TypeScript port of the protocol client with event
  subscription and `last_seq` tracking
- `src/views/*` — conversation, tool activity, verification and diff views
- `src/approvals.ts` — approval prompts wired to `ResolveApproval`
- Integration tests proving CLI↔IDE task continuity (same task id, no
  duplicates)

## The rule that must not be broken

The extension is a **frontend**. It must not reimplement agent logic. It may
not call a model directly, maintain its own task store, or re-implement tool
execution or permission checks. Every action becomes a protocol message; every
piece of state comes from the runtime.

If it ever needs to "just do it locally", that is a design failure — fix the
protocol instead.

## Planned commands

| Command | Protocol request |
| --- | --- |
| APEX: Connect to Runtime | `Hello` |
| APEX: New Task | `CreateTask` |
| APEX: Resume Task | `ResumeTask` |
| APEX: Cancel Task | `CancelTask` |
| APEX: Show Diff | `GetDiff` |