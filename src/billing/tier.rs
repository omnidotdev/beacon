//! Plan tier resolution from Aether entitlements
//!
//! Beacon's managed default model follows the user's plan: the free tier
//! sends the gateway's virtual `auto` model, which the Synapse gateway resolves
//! to Beacon's policy default (a Haiku-class model), and paid tiers keep the
//! configured default. The user can still pick any model the gateway lists for
//! Beacon; credits bound the spend

use synapse_billing::types::EntitlementsResponse;

/// Virtual model the gateway resolves to Beacon's policy default model.
///
/// The free tier sends it instead of a pinned id, so the gateway's beacon
/// policy (not a constant here) decides the free default
pub const GATEWAY_DEFAULT_MODEL: &str = "auto";

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
            Self::Free => GATEWAY_DEFAULT_MODEL.to_string(),
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
    match tier_name(entitlements).as_deref() {
        None | Some("free") => PlanTier::Free,
        Some(_) => PlanTier::Paid,
    }
}

/// The plan tier's name (`free`, `pro`, `team`, ...), lowercased, or `None`
/// when the tier entitlement is missing or empty.
///
/// Aether serves entitlement values as JSON, so the value may be a plain
/// string, JSONB-quoted text (`"\"pro\""`), or a non-string JSON value
#[must_use]
pub fn tier_name(entitlements: &EntitlementsResponse) -> Option<String> {
    let value = entitlements
        .entitlements
        .iter()
        .find(|e| e.feature_key == TIER_FEATURE_KEY)
        .and_then(|e| e.value.as_ref())?;

    let raw = match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    };

    let tier = raw.trim().trim_matches('"').to_ascii_lowercase();

    if tier.is_empty() || tier == "null" {
        None
    } else {
        Some(tier)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(entries: &serde_json::Value) -> EntitlementsResponse {
        serde_json::from_value(serde_json::json!({ "entitlements": entries.clone() })).unwrap()
    }

    #[test]
    fn free_tier_defers_to_the_gateway_default() {
        let r = response(&serde_json::json!([{ "featureKey": "tier", "value": "free" }]));
        let tier = tier_from_entitlements(&r);
        assert_eq!(tier, PlanTier::Free);
        assert_eq!(tier.default_model("anthropic/claude-sonnet-5-5"), "auto");
    }

    #[test]
    fn paid_tiers_keep_the_configured_default() {
        for value in ["pro", "\"team\"", "Pro"] {
            let r = response(&serde_json::json!([
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
    fn tier_name_reads_every_wire_shape() {
        let plain = response(&serde_json::json!([{ "featureKey": "tier", "value": "Pro" }]));
        assert_eq!(tier_name(&plain).as_deref(), Some("pro"));

        let quoted = response(&serde_json::json!([{ "featureKey": "tier", "value": "\"team\"" }]));
        assert_eq!(tier_name(&quoted).as_deref(), Some("team"));

        assert_eq!(tier_name(&response(&serde_json::json!([]))), None);
    }

    #[test]
    fn numeric_limits_as_json_numbers_or_strings_parse_alongside_the_tier() {
        // Aether sends limits as JSON numbers although clients type them as
        // strings; both shapes must deserialize and leave the tier readable
        for limits in [
            serde_json::json!([
                { "featureKey": "max_conversations", "value": -1 },
                { "featureKey": "voice_enabled", "value": 1 },
                { "featureKey": "included_credits", "value": 300 },
                { "featureKey": "tier", "value": "pro" }
            ]),
            serde_json::json!([
                { "featureKey": "max_conversations", "value": "-1" },
                { "featureKey": "voice_enabled", "value": "1" },
                { "featureKey": "included_credits", "value": "300" },
                { "featureKey": "tier", "value": "pro" }
            ]),
        ] {
            let r = response(&limits);
            assert_eq!(tier_name(&r).as_deref(), Some("pro"));
            assert_eq!(tier_from_entitlements(&r), PlanTier::Paid);
        }
    }

    #[test]
    fn jsonb_quoted_free_is_free() {
        let r = response(&serde_json::json!([{ "featureKey": "tier", "value": "\"free\"" }]));
        assert_eq!(tier_from_entitlements(&r), PlanTier::Free);
    }

    #[test]
    fn missing_or_null_tier_is_free() {
        assert_eq!(
            tier_from_entitlements(&response(&serde_json::json!([]))),
            PlanTier::Free
        );
        let r = response(&serde_json::json!([{ "featureKey": "tier", "value": null }]));
        assert_eq!(tier_from_entitlements(&r), PlanTier::Free);
    }
}
