# Contributing to APEX

Thank you for helping. This document covers the ground rules that keep the
codebase coherent.

## Ground rules

1. **Never claim a feature works unless it has been run.** Tests are the
   arbiter. If you cannot test it, say so explicitly in your summary and in the
   documentation.
2. **Keep the project compiling.** Every milestone ends in a green
   `cargo build --workspace` and `cargo test --workspace`.
3. **Small verifiable increments.** A narrow feature that genuinely works beats
   ten incomplete subsystems.
4. **No stubs in production paths.** `todo!()`, `unimplemented!()`, empty match
   arms and "mock responses" do not belong in shipped code. Mocks belong in
   tests only.
5. **Honest documentation.** If a feature is partial, the docs say so. Never
   advertise an integration that has not been tested end to end.

## Language policy

- **Rust** is the primary language for the runtime, CLI, orchestration and
  security-sensitive components.
- **Go** is permitted for optional infrastructure services where it offers a
  concrete advantage.
- **Zig / C / C++** are permitted for low-level or performance-critical parts.
- **Python and Java are prohibited** for runtime services, scripts, build
   orchestration, agent logic or core functionality.

## Before opening a pull request

```bash
cargo fmt --all
cargo clippy --workspace --all-targets
cargo test --workspace
```

## Adding a model provider

1. Implement the `ModelProvider` trait in `apex-models`.
2. Add a `ProviderKind` variant in `apex-core::config`.
3. Register it in `apex_models::build_provider`.
4. Add adapter tests that exercise the request encoding and response parsing
   against the provider's documented contract.
5. Do not claim support in the README until those tests pass against the real
   API.

## Adding a tool

1. Implement the `Tool` trait in `apex-tools`.
2. Give it an accurate `RiskClass` — this is what drives permission enforcement.
3. Register it in `ToolRegistry::default_set`.
4. Route every path through `paths::resolve_within` so the workspace boundary
   cannot be bypassed.
5. Return a structured `ToolResult`; do not force the model to parse raw
   terminal noise.

## Commit messages

Use a conventional prefix:

- `feat:` new capability
- `fix:` bug fix
- `test:` tests only
- `docs:` documentation
- `refactor:` no behaviour change
- `chore:` build, tooling, dependencies

## Reporting bugs

Include the APEX version (`apex --version`), your platform, the project type,
the exact command you ran, and the actual output. Include `apex doctor` output
if the runtime failed to start.