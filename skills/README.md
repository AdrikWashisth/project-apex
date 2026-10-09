# Skills

A **skill** is a reusable, documented procedure an agent can follow. Skills
capture *how to do a recurring kind of work well* — the kind of knowledge that
used to live only in a senior engineer's head.

APEX stores skills as markdown with YAML front matter.

## Format

```markdown
---
name: add-a-rest-endpoint
description: Add a new HTTP endpoint with validation, tests and docs.
agent: apex-default        # optional; which agent should use this skill
version: 1
---

# Add a REST endpoint

1. Find the router and an existing similar endpoint to mirror.
2. Define the request and response types with validation.
3. Register the route with the same auth middleware as its neighbours.
4. Add tests: happy path, validation failure, unauthorized.
5. Run the project's real test command and paste the output.
```

## How skills reach an agent

The intended flow (partially implemented — see the roadmap):

1. Skills live in `~/.apex/skills/` (user) or `<project>/.apex/skills/`.
2. An agent's instructions or a matching description causes retrieval.
3. Retrieved skills are injected into the system prompt as context notes.
4. Task outcomes feed back into skill quality (see Kaizen below).

Today, scoped memory notes (`apex-memory`) are injected as context notes; the
skill file format above is the target shape.

## Writing a good skill

- **Be procedural.** "Do the steps in order" beats "consider best practices".
- **Include the verification step.** Every skill should end with how to prove
  the work is correct.
- **Reference real paths and commands** from the project, not generic advice.
- **Keep it short.** A skill that is too long dilutes the prompt.

## Skills and controlled self-improvement (Kaizen)

Skills are one of the primary things APEX is designed to improve over time:

```text
Execute -> Measure -> Reflect -> Identify improvement -> Create experiment
        -> Evaluate against baseline -> Promote if better -> Record result
```

A proposed skill change is versioned and evaluated against a baseline task
suite before it is promoted. Every promotion and rollback is recorded. An agent
is never allowed to rewrite its own security controls or permissions and deploy
the result unreviewed.

This is specified but not yet built; see [ROADMAP.md](../ROADMAP.md) Milestone 8.