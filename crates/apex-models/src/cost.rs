//! Approximate cost estimation for known models.
//!
//! Prices change frequently. These figures are best-effort estimates used only
//! when a provider does not report exact costs; results are labelled as
//! estimates wherever they surface.

use apex_protocol::Usage;

/// Input USD per million tokens and output USD per million tokens.
struct Price {
    prefix: &'static str,
    input: f64,
    output: f64,
}

const PRICES: &[Price] = &[
    Price {
        prefix: "gpt-4o-mini",
        input: 0.15,
        output: 0.60,
    },
    Price {
        prefix: "gpt-4o",
        input: 2.50,
        output: 10.00,
    },
    Price {
        prefix: "gpt-4.1-mini",
        input: 0.40,
        output: 1.60,
    },
    Price {
        prefix: "gpt-4.1-nano",
        input: 0.10,
        output: 0.40,
    },
    Price {
        prefix: "gpt-4.1",
        input: 2.00,
        output: 8.00,
    },
    Price {
        prefix: "o4-mini",
        input: 1.10,
        output: 4.40,
    },
    Price {
        prefix: "o3-mini",
        input: 1.10,
        output: 4.40,
    },
    Price {
        prefix: "gpt-3.5",
        input: 0.50,
        output: 1.50,
    },
];

/// Estimate the USD cost of a usage record for a model.
///
/// Returns `None` when the model is unknown, so callers can clearly label the
/// result as unavailable instead of guessing.
pub fn estimate_cost(model: &str, usage: &Usage) -> Option<f64> {
    let price = PRICES
        .iter()
        .find(|p| model.starts_with(p.prefix) || model.contains(p.prefix))?;
    let input = usage.prompt_tokens as f64 / 1_000_000.0 * price.input;
    let output = usage.completion_tokens as f64 / 1_000_000.0 * price.output;
    Some(input + output)
}
