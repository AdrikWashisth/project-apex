# Security Policy

## Reporting a vulnerability

Please **do not** open a public issue for a security vulnerability.

Report it privately to the maintainers with:

- The APEX version (`apex --version`)
- Your platform and OS version
- A description of the issue and its impact
- Steps to reproduce, ideally with a minimal repository
- Any proof-of-concept code

We aim to acknowledge reports within three business days.

## What counts as a vulnerability

Given APEX's design, the following are in scope:

- Path traversal or workspace-boundary escape in any tool
- A permission profile failing to enforce its documented restriction
- Secret or credential exposure in logs, config files or events
- Unauthenticated access to the local runtime
- Command injection through the `run_command` tool
- A remote or crafted agent manifest gaining capabilities it did not declare

## What is a known limitation, not a vulnerability

These are documented honestly in [SECURITY_MODEL.md](docs/SECURITY_MODEL.md) and
are engineering backlog, not exploitable defects:

- APEX does not sandbox agent processes; tools run with the user's privileges
- On non-Unix platforms the runtime uses loopback TCP rather than named pipes
- Model output is untrusted and can be steered by hostile repository text;
  permission checks bound the blast radius rather than making the model safe
- Agent manifests are structurally validated but not yet cryptographically
  signed

## Supported versions

| Version | Supported |
| --- | --- |
| 0.1.x | Yes |

## Security posture summary

- Least-privilege tool permissions with four documented profiles
- Explicit filesystem boundary enforced by canonicalisation and prefix checks
- No shell invocation: commands run with an explicit argument vector
- Human approval for configured high-risk operations, deny on timeout
- Secrets read from environment variables only, never persisted or logged
- Bounded retries, output caps, timeouts and resource budgets
- Untrusted-input handling for repository text, model output and manifests