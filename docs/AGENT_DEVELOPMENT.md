# Writing an APEX Agent

An APEX agent is a declarative manifest plus, optionally, reusable skills. It
is not a program. There is no plugin code path, because a published agent must
never be trusted executable code just because it was shared.

## The manifest format

Agents are TOML files. This is the complete shape:

```toml
# --- identity ---
id = "apex-default"          # stable identifier
name = "APEX Default Engineer"
version = "0.1.0"
description = "General-purpose coding agent."
maintainer = "APEX"

# --- behaviour ---
instructions = """
You are APEX, an autonomous software engineering agent.

Operating rules:
1. Inspect before you change.
2. Make focused, minimal edits.
3. Run the project's real build and tests; read the actual output.
4. Never claim success without evidence.
"""

task_types = ["feature", "bugfix", "refactor"]

# --- models ---
[models]
preferred = "openai/gpt-4o-mini"   # optional
fallback = ["openai/gpt-4o"]       # optional, tried in order
requires = ["tools"]               # required capabilities

# --- tools ---
[tools]
allowed = ["read_file", "grep", "edit_file", "run_tests"]
denied  = ["run_command"]           # denied always wins over allowed

# --- execution ---
[execution]
timeout_secs = 1800
max_steps = 60
max_tool_calls = 200
max_tokens = 500000

# --- memory ---
[memory]
scope = "project"          # "project" | "agent" | "user"
persist_lessons = true

# --- evaluation ---
evaluation = ["build-and-test"]
```

An empty `allowed` list means "every tool permitted by the active permission
profile". The permission profile still applies — a manifest cannot widen it.

## Read-only agents and the scheduler

APEX derives whether an agent can modify the workspace **from its own tool
list**, not from anything the caller asserts. An agent is treated as read-only
when it has none of `write_file`, `edit_file`, `run_command`, `run_build` or
`run_tests`.

That has two consequences worth knowing:

1. Read-only agents never conflict, so they always run in parallel with each
   other. `apex-reviewer` is read-only by construction.
2. A workflow **cannot** make a read-only agent writable. Declaring
   `writes = ["src/main.rs"]` on a step whose agent is read-only has no effect —
   the scheduler still treats the step as read-only. This is deliberate: a
   workflow file is configuration, not a privilege escalation.

An agent that *does* hold a mutating tool but is only ever asked to work in one
directory can narrow its blast radius by declaring that scope in the workflow
step's `writes` field. Two agents with provably disjoint scopes will then run
concurrently instead of being serialised.

## Validation

A manifest is rejected before the agent is ever loaded if:

- `id` is empty or contains characters outside `[A-Za-z0-9._-]`
- `name` is empty
- `version` is empty
- `instructions` is empty

Validation is structural. It is not a trust boundary — see
[SECURITY_MODEL.md](SECURITY_MODEL.md).

## Writing good instructions

Instructions are the agent's entire behavioural contract. Effective ones:

- **State the operating rules explicitly.** "Run the tests before claiming
  success" prevents a whole class of unverifiable claims.
- **Forbid fabrication.** Say outright that the agent may not invent command
  output. Models will otherwise try.
- **Describe the method, not just the goal.** A debugger agent that says
  "reproduce, hypothesise, fix, re-run" behaves very differently from one that
  says "fix the bug".
- **Keep them short.** A wall of prose dilutes itself. The built-in agents are
  10–15 lines.

## The three built-in agents

| Agent | Purpose | Tools |
| --- | --- | --- |
| `apex-default` | General coding work | all |
| `apex-reviewer` | Read-only review, no edits | read/search/git only |
| `apex-reviewer` denies | `write_file`, `edit_file`, `run_command`, `run_build`, `run_tests` | — |
| `apex-debugger` | Reproduce, diagnose, repair | all |

## Composing agents

A team is just a list of agent ids. Sequential (the default) chains them so each
one receives the previous one's actual findings:

```bash
apex run "Harden the HTTP layer" --agents apex-debugger,apex-reviewer
```

A workflow gives finer control, including ordering and write scopes:

```toml
name = "review-then-fix"

[[steps]]
id = "review"
agent = "apex-reviewer"      # read-only → runs in wave 0
objective = "Report concrete problems, ordered by severity."

[[steps]]
id = "fix"
agent = "apex-debugger"
objective = "Fix what the review found."
depends_on = ["review"]       # runs in wave 1, with the review in context
```

Preview any workflow without spending anything:

```bash
apex agents plan workflows/review-then-fix.toml
```

## Test an agent

Because manifests are declarative, you can exercise an agent deterministically
with the fake provider:

1. Write the manifest.
2. Script a `FakeProvider` with the tool calls you expect.
3. Run `AgentRunner::run` against a temporary Git repository.
4. Assert the tools ran, the files changed inside the workspace, and that
   verification reports the real outcome.

This is exactly the pattern in `tests/tests/e2e_vertical_slice.rs`.

## Publishing

Not yet implemented. When it lands, publishing will require: a signed manifest,
a declared permission set, a provenance record, and evaluation results. An agent
that carries executable extensions will run with explicit permissions and an
isolation boundary — installation will never silently grant filesystem, network
or shell access.