# Model Providers

All model access goes through one interface, `apex_models::ModelProvider`.
Nothing in APEX hard-codes a specific vendor.

## The interface

```rust
#[async_trait]
pub trait ModelProvider: Send + Sync {
    fn name(&self) -> &str;
    fn capabilities(&self, model: &str) -> ModelCapabilities;
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse>;
}
```

`ModelCapabilities` reports, per model, whether tools are supported, whether
streaming is available, and the context and output limits. APEX reads these
rather than assuming every model behaves the same way, and rejects unsupported
features explicitly instead of silently sending a request that will fail.

## Built-in adapters

### `openai_compatible`

Implements `POST {base_url}/chat/completions`. Works with the OpenAI API and
with any server exposing the same contract, including most local model servers.

- Encodes system/user/assistant/tool messages and the `tools` array.
- Decodes `tool_calls`, `finish_reason` and `usage`.
- Retries `429` and `5xx` with exponential backoff, capped at three attempts.
- Surfaces non-retryable HTTP status codes with the endpoint and a truncated
  body so the failure is diagnosable.

Configure it like this:

```toml
default_model = "openai/gpt-4o-mini"

[providers.openai]
kind = "openai_compatible"
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"
models = ["gpt-4o-mini", "gpt-4o"]
default_model = "gpt-4o-mini"
```

### `fake`

Deterministic and offline. It performs no network I/O and exists so the CLI,
the test suite and the demos work without credentials.

```toml
default_model = "local/offline"

[providers.local]
kind = "fake"
base_url = "http://localhost"
models = ["offline"]
```

A `fake` provider cannot perform real work. When one is configured, APEX will
fail verification and report the task as failed — that is the system working
correctly, not a bug.

## Credentials

API keys are read from the environment variable named by `api_key_env`. They
are never written to the config file, never printed, and never included in
events or logs. Do not put a key directly in a manifest or a project config.

## Adding a provider

1. Implement `ModelProvider` for your adapter in `apex-models`.
2. Add a `ProviderKind` variant in `apex-core::config`.
3. Register it in `apex_models::build_provider`.
4. Write adapter tests that exercise request encoding and response parsing
   against the provider's documented contract.
5. Only claim the integration works once those tests pass against the real API.

Do not advertise an integration that has not been tested end to end.

## Cost and usage

Token usage comes from the provider's `usage` block when it reports one. When a
provider does not report cost, APEX estimates it from a small built-in price
table and labels the figure as an estimate. Unknown models return no cost
rather than a guess, so the CLI can say "unavailable" instead of inventing a
number.

Prices change frequently; the table in `apex-models/src/cost.rs` is
best-effort and should be refreshed periodically.

## Model routing

The current router resolves a `provider/model` reference and falls back through
the user's default. Capability-based routing, latency-aware selection and
explicit privacy routing are on the roadmap (Milestone 9).

One rule is already enforced: **APEX does not silently switch providers when a
task contains private code.** If the configured provider is unavailable, the
task fails with a provider error rather than quietly sending the code elsewhere.

## Streaming

`ModelCapabilities::supports_streaming` reports whether a provider can stream.
Streaming transports are not yet wired through the execution loop; the runtime
streams *task events* live over IPC, which is the part clients depend on today.