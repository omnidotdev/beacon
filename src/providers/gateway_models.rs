//! Beacon's managed model list, served by the Synapse gateway
//!
//! The gateway owns Beacon's model policy: a product-scoped key's
//! `GET /v1/models` lists exactly the models Beacon may use, in policy order,
//! with display names, the policy default, and retail credit prices. The
//! picker shows that list instead of a hardcoded one, and the default it marks
//! follows the user's plan: the gateway default on the free tier, the
//! configured paid default on paid tiers

use std::sync::LazyLock;
use std::time::Duration;

use serde::Serialize;

use crate::billing::PlanTier;

/// Timeout for the listing call; the picker degrades when it fails
const LIST_TIMEOUT: Duration = Duration::from_secs(10);

static HTTP: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(LIST_TIMEOUT)
        .build()
        .unwrap_or_default()
});

/// Retail price in AI credits per million tokens, as the gateway bills
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct CreditsPerMtok {
    pub input: f64,
    pub output: f64,
}

/// One chat model from the gateway's listing
#[derive(Debug, Clone, PartialEq)]
pub struct GatewayModel {
    /// Provider-prefixed id the gateway routes by
    pub id: String,
    /// Human-readable name (falls back to the id)
    pub name: String,
    /// Whether the gateway marks it as the product default
    pub is_default: bool,
    /// Context window in tokens, when known
    pub context_window: Option<u64>,
    /// Retail credits per million tokens; `None` when the caller is not billed
    pub credits_per_mtok: Option<CreditsPerMtok>,
}

/// A model as the picker shows it
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PickerModel {
    pub id: String,
    pub name: String,
    /// Whether this is the model the user's turns run on when nothing is picked
    pub default: bool,
    pub context_window: Option<u64>,
    pub credits_per_mtok: Option<CreditsPerMtok>,
}

/// Response for `GET /api/providers/models`
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModelsResponse {
    /// Managed models the user may pick, in the gateway's policy order
    pub models: Vec<PickerModel>,
    /// Id of the model turns run on when nothing is picked, if listed
    pub default_model: Option<String>,
}

impl ModelsResponse {
    /// No managed models (managed AI is not available here)
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            models: Vec::new(),
            default_model: None,
        }
    }
}

/// Parse a `GET /v1/models` body (`OpenAI` list shape plus the `synapse`
/// extension). Entries without a string id are skipped
#[must_use]
pub fn parse_model_list(body: &serde_json::Value) -> Vec<GatewayModel> {
    let Some(data) = body.get("data").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };

    data.iter()
        .filter_map(|entry| {
            let id = entry.get("id")?.as_str()?.to_string();
            let ext = entry.get("synapse");
            let field = |name: &str| ext.and_then(|e| e.get(name));

            let name = field("display_name")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .map_or_else(|| id.clone(), str::to_string);
            let is_default = field("default")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let context_window = field("context_window").and_then(serde_json::Value::as_u64);
            let credits_per_mtok = field("credits_per_mtok").and_then(|price| {
                Some(CreditsPerMtok {
                    input: price.get("input")?.as_f64()?,
                    output: price.get("output")?.as_f64()?,
                })
            });

            Some(GatewayModel {
                id,
                name,
                is_default,
                context_window,
                credits_per_mtok,
            })
        })
        .collect()
}

/// Build the picker response for a user on `tier`.
///
/// The free tier runs on the gateway's default (the entry the gateway marks,
/// else the first listed). Paid tiers run on `paid_default` when the gateway
/// lists it, else on the gateway's default, so the marked default is always a
/// model the gateway will serve
#[must_use]
pub fn picker_response(
    models: Vec<GatewayModel>,
    tier: PlanTier,
    paid_default: &str,
) -> ModelsResponse {
    let gateway_default = models
        .iter()
        .find(|m| m.is_default)
        .or_else(|| models.first())
        .map(|m| m.id.clone());

    let default_model = match tier {
        PlanTier::Paid if models.iter().any(|m| m.id == paid_default) => {
            Some(paid_default.to_string())
        }
        _ => gateway_default,
    };

    let models = models
        .into_iter()
        .map(|m| PickerModel {
            default: default_model.as_deref() == Some(m.id.as_str()),
            id: m.id,
            name: m.name,
            context_window: m.context_window,
            credits_per_mtok: m.credits_per_mtok,
        })
        .collect();

    ModelsResponse {
        models,
        default_model,
    }
}

/// Fetch the models a product-scoped key may use from the gateway
///
/// # Errors
///
/// Returns an error when the gateway is unreachable, refuses the key, or
/// returns a body that is not JSON
pub async fn fetch_gateway_models(
    base_url: &str,
    api_key: &str,
) -> anyhow::Result<Vec<GatewayModel>> {
    let url = format!("{}/v1/models", base_url.trim_end_matches('/'));
    let response = HTTP.get(&url).bearer_auth(api_key).send().await?;

    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("gateway model listing returned {status}");
    }

    let body: serde_json::Value = response.json().await?;
    Ok(parse_model_list(&body))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing() -> serde_json::Value {
        serde_json::json!({
            "object": "list",
            "data": [
                {
                    "id": "anthropic/claude-haiku-5-5",
                    "object": "model",
                    "created": 0,
                    "owned_by": "anthropic",
                    "synapse": {
                        "display_name": "Claude Haiku 5.5",
                        "default": true,
                        "context_window": 1_000_000,
                        "credits_per_mtok": { "input": 20.0, "output": 100.0 }
                    }
                },
                {
                    "id": "anthropic/claude-sonnet-5-5",
                    "object": "model",
                    "synapse": {
                        "display_name": "Claude Sonnet 5.5",
                        "default": false,
                        "context_window": 1_000_000,
                        "credits_per_mtok": { "input": 400.0, "output": 2000.0 }
                    }
                },
                {
                    "id": "anthropic/claude-opus-5-5",
                    "object": "model",
                    "synapse": { "display_name": "", "default": false, "credits_per_mtok": null }
                },
                { "object": "model" }
            ]
        })
    }

    #[test]
    fn parses_the_synapse_extension() {
        let models = parse_model_list(&listing());
        assert_eq!(models.len(), 3, "entries without an id are skipped");

        let haiku = &models[0];
        assert_eq!(haiku.id, "anthropic/claude-haiku-5-5");
        assert_eq!(haiku.name, "Claude Haiku 5.5");
        assert!(haiku.is_default);
        assert_eq!(haiku.context_window, Some(1_000_000));
        assert_eq!(
            haiku.credits_per_mtok,
            Some(CreditsPerMtok {
                input: 20.0,
                output: 100.0
            })
        );

        let opus = &models[2];
        assert_eq!(
            opus.name, "anthropic/claude-opus-5-5",
            "blank name falls back to the id"
        );
        assert!(!opus.is_default);
        assert_eq!(opus.credits_per_mtok, None, "unbilled callers get no price");
    }

    #[test]
    fn plain_openai_listing_still_parses() {
        let body = serde_json::json!({ "data": [{ "id": "gpt-4o", "object": "model" }] });
        let models = parse_model_list(&body);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].name, "gpt-4o");
        assert!(!models[0].is_default);
    }

    #[test]
    fn malformed_body_lists_nothing() {
        assert!(parse_model_list(&serde_json::json!({ "error": "nope" })).is_empty());
    }

    #[test]
    fn free_tier_marks_the_gateway_default() {
        let response = picker_response(
            parse_model_list(&listing()),
            PlanTier::Free,
            "anthropic/claude-sonnet-5-5",
        );
        assert_eq!(
            response.default_model.as_deref(),
            Some("anthropic/claude-haiku-5-5")
        );
        let defaults: Vec<_> = response.models.iter().filter(|m| m.default).collect();
        assert_eq!(defaults.len(), 1);
        assert_eq!(defaults[0].id, "anthropic/claude-haiku-5-5");
    }

    #[test]
    fn paid_tier_marks_the_paid_default_when_listed() {
        let response = picker_response(
            parse_model_list(&listing()),
            PlanTier::Paid,
            "anthropic/claude-sonnet-5-5",
        );
        assert_eq!(
            response.default_model.as_deref(),
            Some("anthropic/claude-sonnet-5-5")
        );
        assert!(response.models.iter().filter(|m| m.default).count() == 1);
    }

    #[test]
    fn paid_default_outside_the_policy_falls_back_to_the_gateway_default() {
        let response = picker_response(
            parse_model_list(&listing()),
            PlanTier::Paid,
            "openai/gpt-4o",
        );
        assert_eq!(
            response.default_model.as_deref(),
            Some("anthropic/claude-haiku-5-5")
        );
    }

    #[test]
    fn listing_without_a_marked_default_uses_the_first_model() {
        let body = serde_json::json!({
            "data": [{ "id": "a/one" }, { "id": "a/two" }]
        });
        let response = picker_response(parse_model_list(&body), PlanTier::Free, "a/two");
        assert_eq!(response.default_model.as_deref(), Some("a/one"));
    }

    #[test]
    fn empty_listing_has_no_default() {
        let response = picker_response(Vec::new(), PlanTier::Paid, "a/two");
        assert_eq!(response, ModelsResponse::empty());
    }

    #[test]
    fn response_serializes_for_the_app() {
        let response = picker_response(
            parse_model_list(&listing()),
            PlanTier::Free,
            "anthropic/claude-sonnet-5-5",
        );
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["default_model"], "anthropic/claude-haiku-5-5");
        assert_eq!(json["models"][0]["name"], "Claude Haiku 5.5");
        assert_eq!(json["models"][0]["default"], true);
        assert_eq!(json["models"][0]["credits_per_mtok"]["input"], 20.0);
        assert!(json["models"][2]["credits_per_mtok"].is_null());
    }
}
