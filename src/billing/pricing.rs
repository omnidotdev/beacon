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

/// Look up pricing for a model, falling back to a reasonable default
#[must_use]
pub fn lookup_pricing(model: &str) -> ModelPricing {
    // Normalize model name for matching
    let lower = model.to_lowercase();

    // Claude models
    if lower.contains("opus") {
        return ModelPricing {
            input_per_million: 15.0,
            output_per_million: 75.0,
            cache_read_per_million: Some(1.875),
        };
    }
    if lower.contains("sonnet") {
        return ModelPricing {
            input_per_million: 3.0,
            output_per_million: 15.0,
            cache_read_per_million: Some(0.30),
        };
    }
    if lower.contains("haiku") {
        return ModelPricing {
            input_per_million: 0.80,
            output_per_million: 4.0,
            cache_read_per_million: Some(0.08),
        };
    }

    // OpenAI models
    if lower.contains("gpt-4o-mini") {
        return ModelPricing {
            input_per_million: 0.15,
            output_per_million: 0.60,
            cache_read_per_million: Some(0.075),
        };
    }
    if lower.contains("gpt-4o") {
        return ModelPricing {
            input_per_million: 2.50,
            output_per_million: 10.0,
            cache_read_per_million: Some(1.25),
        };
    }
    if lower.contains("o3-mini") {
        return ModelPricing {
            input_per_million: 1.10,
            output_per_million: 4.40,
            cache_read_per_million: Some(0.55),
        };
    }
    if lower.contains("o3") {
        return ModelPricing {
            input_per_million: 10.0,
            output_per_million: 40.0,
            cache_read_per_million: None,
        };
    }
    if lower.contains("o4-mini") {
        return ModelPricing {
            input_per_million: 1.10,
            output_per_million: 4.40,
            cache_read_per_million: Some(0.275),
        };
    }

    // Google models
    if lower.contains("gemini-2.5-pro") {
        return ModelPricing {
            input_per_million: 1.25,
            output_per_million: 10.0,
            cache_read_per_million: None,
        };
    }
    if lower.contains("gemini-2.5-flash") {
        return ModelPricing {
            input_per_million: 0.15,
            output_per_million: 0.60,
            cache_read_per_million: None,
        };
    }

    // Default fallback (conservative estimate)
    ModelPricing {
        input_per_million: 3.0,
        output_per_million: 15.0,
        cache_read_per_million: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_cost_claude_sonnet() {
        let cost = estimate_cost("claude-sonnet-4-20250514", 1000, 500);
        // 1000/1M * 3.0 + 500/1M * 15.0 = 0.003 + 0.0075 = 0.0105
        assert!((cost - 0.0105).abs() < 0.0001);
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
