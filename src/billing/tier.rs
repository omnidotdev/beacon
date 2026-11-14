//! Plan tier resolution from Aether entitlements
//!
//! Beacon's managed default model follows the user's plan: the free tier
//! defaults to a Haiku-class model, paid tiers keep the configured default.
//! The user can still pick any model explicitly; credits bound the spend

use synapse_billing::types::EntitlementsResponse;

/// Default managed model for the free tier
pub const FREE_TIER_MODEL: &str = "anthropic/claude-haiku-5-5";

/// Entitlement feature key carrying the plan tier
const TIER_FEATURE_KEY: &str = "tier";

/// Coarse plan tier, as far as model defaults are concerned
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanTier {
    /// Free tier, or a tier that could not be determined
    Free,
    /// Any paid tier (pro, team, ...)
    Paid,
}

impl PlanTier {
    /// Default model for this tier, given the configured paid default
    #[must_use]
    pub fn default_model(self, paid_default: &str) -> String {
        match self {
            Self::Free => FREE_TIER_MODEL.to_string(),
            Self::Paid => paid_default.to_string(),
        }
    }
}

/// Read the plan tier from an entitlements response.
///
/// A missing or unreadable tier resolves to `Free`, so an unknown account
/// never defaults to the more expensive model
#[must_use]
pub fn tier_from_entitlements(entitlements: &EntitlementsResponse) -> PlanTier {
    let Some(value) = entitlements
        .entitlements
        .iter()
        .find(|e| e.feature_key == TIER_FEATURE_KEY)
        .and_then(|e| e.value.as_ref())
    else {
        return PlanTier::Free;
    };

    let raw = match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    };

    // Aether may hand back JSONB-quoted text (`"\"pro\""`)
    let tier = raw.trim().trim_matches('"').to_ascii_lowercase();

    if tier.is_empty() || tier == "free" || tier == "null" {
        PlanTier::Free
    } else {
        PlanTier::Paid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(entries: serde_json::Value) -> EntitlementsResponse {
        serde_json::from_value(serde_json::json!({ "entitlements": entries })).unwrap()
    }

    #[test]
    fn free_tier_defaults_to_haiku() {
        let r = response(serde_json::json!([{ "featureKey": "tier", "value": "free" }]));
        let tier = tier_from_entitlements(&r);
        assert_eq!(tier, PlanTier::Free);
        assert_eq!(
            tier.default_model("anthropic/claude-sonnet-5-5"),
            "anthropic/claude-haiku-5-5"
        );
    }

    #[test]
    fn paid_tiers_keep_the_configured_default() {
        for value in ["pro", "\"team\"", "Pro"] {
            let r = response(serde_json::json!([
                { "featureKey": "api_access", "value": "true" },
                { "featureKey": "tier", "value": value }
            ]));
            let tier = tier_from_entitlements(&r);
            assert_eq!(tier, PlanTier::Paid, "{value}");
            assert_eq!(
                tier.default_model("anthropic/claude-sonnet-5-5"),
                "anthropic/claude-sonnet-5-5"
            );
        }
    }

    #[test]
    fn jsonb_quoted_free_is_free() {
        let r = response(serde_json::json!([{ "featureKey": "tier", "value": "\"free\"" }]));
        assert_eq!(tier_from_entitlements(&r), PlanTier::Free);
    }

    #[test]
    fn missing_or_null_tier_is_free() {
        assert_eq!(
            tier_from_entitlements(&response(serde_json::json!([]))),
            PlanTier::Free
        );
        let r = response(serde_json::json!([{ "featureKey": "tier", "value": null }]));
        assert_eq!(tier_from_entitlements(&r), PlanTier::Free);
    }
}
