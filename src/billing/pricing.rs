//! Model pricing catalog for local cost estimation

/// Pricing rates for a model (per million tokens)
#[derive(Debug, Clone)]
pub struct ModelPricing {
    /// Cost per million input tokens (USD)
    pub input_per_million: f64,
    /// Cost per million output tokens (USD)
    pub output_per_million: f64,
    /// Cost per million cached/read tokens (USD), if applicable
    pub cache_read_per_million: Option<f64>,
}

/// Estimate the cost of a model invocation
#[must_use]
#[allow(clippy::cast_lossless)]
pub fn estimate_cost(model: &str, input_tokens: u32, output_tokens: u32) -> f64 {
    let pricing = lookup_pricing(model);
    let input_cost = (input_tokens as f64 / 1_000_000.0) * pricing.input_per_million;
    let output_cost = (output_tokens as f64 / 1_000_000.0) * pricing.output_per_million;
    input_cost + output_cost
}

/// Build a rate entry (USD per million tokens)
const fn rate(input: f64, output: f64, cache_read: Option<f64>) -> ModelPricing {
    ModelPricing {
        input_per_million: input,
        output_per_million: output,
        cache_read_per_million: cache_read,
    }
}

/// Known model rates, matched by substring in order
///
/// Order matters: each Claude family name repeats across generations at
/// different prices, so newer generations sit ahead of the bare family
/// fallback, and `gpt-4o-mini` / `o3-mini` sit ahead of `gpt-4o` / `o3`
const RATES: &[(&[&str], ModelPricing)] = &[
    // Claude
    (&["opus-5-5"], rate(4.0, 20.0, Some(0.40))),
    (
        &["opus-5", "opus-4-8", "opus-4-6", "opus-4-5"],
        rate(5.0, 25.0, Some(0.50)),
    ),
    (&["opus"], rate(15.0, 75.0, Some(1.50))),
    (&["sonnet-5"], rate(2.0, 10.0, Some(0.20))),
    (&["sonnet"], rate(3.0, 15.0, Some(0.30))),
    // Base rate; prompts over 100K tokens bill at a higher tier upstream
    (&["haiku-5"], rate(0.10, 0.50, Some(0.01))),
    (&["haiku-4-5"], rate(1.0, 5.0, Some(0.10))),
    (&["haiku"], rate(0.80, 4.0, Some(0.08))),
    // OpenAI
    (&["gpt-4o-mini"], rate(0.15, 0.60, Some(0.075))),
    (&["gpt-4o"], rate(2.50, 10.0, Some(1.25))),
    (&["o3-mini"], rate(1.10, 4.40, Some(0.55))),
    (&["o3"], rate(10.0, 40.0, None)),
    (&["o4-mini"], rate(1.10, 4.40, Some(0.275))),
    // Google
    (&["gemini-2.5-pro"], rate(1.25, 10.0, None)),
    (&["gemini-2.5-flash"], rate(0.15, 0.60, None)),
];

/// Conservative fallback for unknown models
const DEFAULT_RATE: ModelPricing = rate(3.0, 15.0, None);

/// Look up pricing for a model, falling back to a reasonable default
#[must_use]
pub fn lookup_pricing(model: &str) -> ModelPricing {
    let lower = model.to_lowercase();

    RATES
        .iter()
        .find(|(needles, _)| needles.iter().any(|needle| lower.contains(needle)))
        .map_or(DEFAULT_RATE, |(_, pricing)| pricing.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_cost_claude_sonnet() {
        let cost = estimate_cost("claude-sonnet-5-5", 1000, 500);
        // 1000/1M * 2.0 + 500/1M * 10.0 = 0.002 + 0.005 = 0.007
        assert!((cost - 0.007).abs() < 0.0001);
    }

    fn rates(model: &str) -> (f64, f64) {
        let p = lookup_pricing(model);
        (p.input_per_million, p.output_per_million)
    }

    #[test]
    fn specific_ids_win_over_family_fallbacks() {
        let cases = [
            ("anthropic/claude-opus-5-5", (4.0, 20.0)),
            ("claude-opus-5", (5.0, 25.0)),
            ("claude-opus-4-8", (5.0, 25.0)),
            ("claude-opus-4-6", (5.0, 25.0)),
            ("claude-3-opus-20240229", (15.0, 75.0)),
            ("anthropic/claude-sonnet-5-5", (2.0, 10.0)),
            ("claude-sonnet-5", (2.0, 10.0)),
            ("claude-sonnet-4-5-20250929", (3.0, 15.0)),
            ("claude-haiku-5-5", (0.10, 0.50)),
            ("claude-haiku-4-5-20251001", (1.0, 5.0)),
            ("claude-3-5-haiku-20241022", (0.80, 4.0)),
            ("gpt-4o-mini", (0.15, 0.60)),
            ("o3-mini", (1.10, 4.40)),
            ("o3", (10.0, 40.0)),
        ];
        for (model, (input, output)) in cases {
            let (got_in, got_out) = rates(model);
            assert!(
                (got_in - input).abs() < f64::EPSILON && (got_out - output).abs() < f64::EPSILON,
                "{model}: got ({got_in}, {got_out}), want ({input}, {output})"
            );
        }
    }

    #[test]
    fn estimate_cost_gpt4o() {
        let cost = estimate_cost("gpt-4o", 1_000_000, 1_000_000);
        // 1.0 * 2.5 + 1.0 * 10.0 = 12.5
        assert!((cost - 12.5).abs() < 0.01);
    }

    #[test]
    fn unknown_model_uses_default() {
        let pricing = lookup_pricing("some-unknown-model");
        assert!((pricing.input_per_million - 3.0).abs() < f64::EPSILON);
    }
}
