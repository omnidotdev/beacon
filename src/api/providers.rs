//! Provider configuration API for BYOK (Bring Your Own Key)
//!
//! Lists available LLM providers and their status.
//! - **Cloud**: key management is via the Synapse dashboard; keys resolved per-user at request time
//! - **Self-hosted**: keys stored locally in `SQLite` via `POST /configure` and `DELETE /{provider}`

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::HeaderMap,
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};

use super::ApiState;
use crate::providers::gateway_models::{ModelsResponse, fetch_gateway_models, picker_response};

/// Available LLM provider types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderType {
    /// `OpenAI` API (GPT models)
    Openai,
    /// Anthropic API (Claude models)
    Anthropic,
    /// `OpenRouter` (aggregated access to multiple providers)
    Openrouter,
    /// AI credits (pay-per-use via Synapse router)
    OmniCredits,
}

/// Provider configuration status
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderStatus {
    /// Provider is configured and ready
    Configured,
    /// Provider is not configured (no API key)
    NotConfigured,
    /// Provider is coming soon
    ComingSoon,
    /// Provider configuration is invalid
    Invalid,
}

/// Provider information returned to clients
#[derive(Debug, Clone, Serialize)]
pub struct ProviderInfo {
    pub id: ProviderType,
    pub name: String,
    pub description: String,
    pub status: ProviderStatus,
    /// Whether this provider is currently active
    pub active: bool,
    /// URL for getting an API key (for BYOK providers)
    pub api_key_url: Option<String>,
    /// Whether this provider is coming soon
    pub coming_soon: bool,
    /// Features available with this provider
    pub features: Vec<String>,
}

/// All providers response
#[derive(Debug, Serialize)]
pub struct ProvidersResponse {
    pub providers: Vec<ProviderInfo>,
    pub active_provider: Option<ProviderType>,
}

/// Request body for POST /api/providers/configure
#[derive(Debug, Deserialize)]
pub struct ConfigureRequest {
    pub provider: String,
    pub api_key: String,
    pub model_preference: Option<String>,
}

/// Response for configure/remove operations
#[derive(Debug, Serialize)]
pub struct ConfigureResponse {
    pub success: bool,
    pub message: String,
}

/// Extract user ID from JWT in the Authorization header
async fn extract_user_id(headers: &HeaderMap, state: &ApiState) -> Option<String> {
    let Some(jwt_cache) = state.jwt_cache.as_ref() else {
        tracing::warn!(
            "BYOK auth failed: no JWT cache (SYNAPSE_API_URL or SYNAPSE_GATEWAY_SECRET not set)"
        );
        return None;
    };

    let Some(auth) = headers.get("authorization").and_then(|v| v.to_str().ok()) else {
        tracing::warn!("BYOK auth failed: no Authorization header in request");
        return None;
    };

    let Some(token) = auth.strip_prefix("Bearer ") else {
        tracing::warn!("BYOK auth failed: Authorization header is not Bearer scheme");
        return None;
    };

    match jwt_cache.validate(token).await {
        Ok(claims) => Some(claims.sub),
        Err(e) => {
            tracing::warn!(error = %e, "BYOK auth failed: JWT validation error");
            None
        }
    }
}

/// Resolve provider status for a specific provider
///
/// Priority: user-configured Synapse key → local DB key → env var
fn provider_status(
    provider_str: &str,
    user_configured: &[String],
    local_configured: &[String],
) -> ProviderStatus {
    if user_configured.contains(&provider_str.to_string()) {
        return ProviderStatus::Configured;
    }
    if local_configured.contains(&provider_str.to_string()) {
        return ProviderStatus::Configured;
    }
    // Check env vars directly for all providers
    let has_env_key = match provider_str {
        "anthropic" => std::env::var("ANTHROPIC_API_KEY").is_ok(),
        "openai" => std::env::var("OPENAI_API_KEY").is_ok(),
        "openrouter" => std::env::var("OPENROUTER_API_KEY").is_ok(),
        _ => false,
    };
    if has_env_key {
        ProviderStatus::Configured
    } else {
        ProviderStatus::NotConfigured
    }
}

/// Get all available providers and their status
#[allow(clippy::too_many_lines)]
async fn list_providers(
    headers: HeaderMap,
    State(state): State<Arc<ApiState>>,
) -> Json<ProvidersResponse> {
    // Check per-user configured providers if authenticated
    let user_id = extract_user_id(&headers, &state).await;
    let user_configured = if let Some(ref uid) = user_id {
        if let Some(resolver) = &state.key_resolver {
            resolver.list_configured(uid).await
        } else {
            vec![]
        }
    } else {
        vec![]
    };

    let local_configured = state
        .local_key_store
        .as_ref()
        .map_or_else(Vec::new, |store| {
            store.list_configured().unwrap_or_default()
        });

    let preferred_provider = if let Some(ref uid) = user_id {
        if let Some(resolver) = &state.key_resolver {
            resolver
                .resolve_preferred(uid)
                .await
                .ok()
                .flatten()
                .map(|(provider, _)| provider)
        } else {
            None
        }
    } else {
        None
    };

    let openai_status = provider_status("openai", &user_configured, &local_configured);
    let anthropic_status = provider_status("anthropic", &user_configured, &local_configured);
    let openrouter_status = provider_status("openrouter", &user_configured, &local_configured);

    let providers = vec![
        ProviderInfo {
            id: ProviderType::Openai,
            name: "OpenAI".to_string(),
            description: "GPT and other OpenAI models".to_string(),
            status: openai_status,
            active: preferred_provider.as_deref() == Some("openai"),
            api_key_url: Some("https://platform.openai.com/api-keys".to_string()),
            coming_soon: false,
            features: vec![
                "Chat completions".to_string(),
                "Whisper STT".to_string(),
                "TTS".to_string(),
            ],
        },
        ProviderInfo {
            id: ProviderType::Anthropic,
            name: "Anthropic".to_string(),
            description: "Claude Opus, Sonnet, and Haiku models".to_string(),
            status: anthropic_status,
            active: preferred_provider.as_deref() == Some("anthropic"),
            api_key_url: Some("https://console.anthropic.com/settings/keys".to_string()),
            coming_soon: false,
            features: vec![
                "Chat completions".to_string(),
                "Tool use".to_string(),
                "Extended context".to_string(),
            ],
        },
        ProviderInfo {
            id: ProviderType::Openrouter,
            name: "OpenRouter".to_string(),
            description: "Access 500+ models from all major providers with one API key".to_string(),
            status: openrouter_status,
            active: preferred_provider.as_deref() == Some("openrouter"),
            api_key_url: Some("https://openrouter.ai/keys".to_string()),
            coming_soon: false,
            features: vec![
                "500+ models".to_string(),
                "Unified billing".to_string(),
                "Automatic fallbacks".to_string(),
            ],
        },
        {
            let synapse_available = state.synapse.is_some();
            let provisioner_available = state.key_provisioner.is_some();
            let has_cached_key = user_configured.contains(&"omni_credits".to_string());

            let (omni_status, omni_coming_soon) = if synapse_available && provisioner_available {
                if user_id.is_some() || has_cached_key {
                    (ProviderStatus::Configured, false)
                } else {
                    (ProviderStatus::NotConfigured, false)
                }
            } else if synapse_available {
                (ProviderStatus::NotConfigured, false)
            } else {
                (ProviderStatus::ComingSoon, true)
            };

            ProviderInfo {
                id: ProviderType::OmniCredits,
                name: "AI Credits".to_string(),
                description: "Omni's AI gateway with current Claude models and MCP support. No API keys needed".to_string(),
                status: omni_status,
                active: preferred_provider.as_deref() == Some("omni_credits"),
                api_key_url: None,
                coming_soon: omni_coming_soon,
                features: vec![
                    "Claude Haiku, Sonnet, and Opus".to_string(),
                    "MCP server aggregation".to_string(),
                    "Tool discovery".to_string(),
                ],
            }
        },
    ];

    // Active provider is the user's explicit preference (from Synapse defaultProvider)
    let active_provider = preferred_provider.as_ref().map_or_else(
        || {
            // No Synapse preference — use local store or env order: anthropic → openai → openrouter
            let all_configured: Vec<&str> = ["anthropic", "openai", "openrouter"]
                .into_iter()
                .filter(|p| {
                    local_configured.contains(&p.to_string())
                        || std::env::var(match *p {
                            "anthropic" => "ANTHROPIC_API_KEY",
                            "openai" => "OPENAI_API_KEY",
                            _ => "OPENROUTER_API_KEY",
                        })
                        .is_ok()
                })
                .collect();

            all_configured.first().and_then(|p| match *p {
                "anthropic" => Some(ProviderType::Anthropic),
                "openai" => Some(ProviderType::Openai),
                "openrouter" => Some(ProviderType::Openrouter),
                _ => None,
            })
        },
        |pref| match pref.as_str() {
            "anthropic" => Some(ProviderType::Anthropic),
            "openai" => Some(ProviderType::Openai),
            "openrouter" => Some(ProviderType::Openrouter),
            "omni_credits" => Some(ProviderType::OmniCredits),
            _ => None,
        },
    );

    Json(ProvidersResponse {
        providers,
        active_provider,
    })
}

/// Error body the app reads (`error.code`, `error.message`)
fn models_error(
    status: axum::http::StatusCode,
    code: &str,
    message: &str,
) -> (axum::http::StatusCode, Json<serde_json::Value>) {
    (
        status,
        Json(serde_json::json!({ "error": { "code": code, "message": message } })),
    )
}

/// Managed models the signed-in user may pick, from the Synapse gateway's
/// Beacon policy, with the default marked for the user's plan
///
/// Cloud mode only: managed AI runs on the user's product-scoped key there.
/// Elsewhere there are no managed models and the list is empty
async fn list_models(
    headers: HeaderMap,
    State(state): State<Arc<ApiState>>,
) -> Result<Json<ModelsResponse>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    use axum::http::StatusCode;

    if !state.cloud_mode {
        return Ok(Json(ModelsResponse::empty()));
    }

    let Some(user_id) = extract_user_id(&headers, &state).await else {
        return Err(models_error(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Sign in to see the available models",
        ));
    };

    let key = super::managed::managed_gateway_key(&state, &user_id)
        .await
        .map_err(|e| models_error(e.status(), e.code(), &e.user_message()))?;

    let base_url = super::managed::synapse_base_url(&state);
    let models = fetch_gateway_models(&base_url, &key).await.map_err(|e| {
        tracing::warn!(error = %e, user_id = %user_id, "gateway model listing failed");
        let failure = super::managed::ManagedError::Unavailable;
        models_error(
            StatusCode::BAD_GATEWAY,
            failure.code(),
            &failure.user_message(),
        )
    })?;

    let tier = match state.billing_state.as_ref() {
        Some(billing) => billing.user_tier(&user_id).await,
        None => crate::billing::PlanTier::Free,
    };

    Ok(Json(picker_response(models, tier, &state.llm_model)))
}

/// Configure a provider key locally (self-hosted deployments)
async fn configure_provider(
    headers: HeaderMap,
    State(state): State<Arc<ApiState>>,
    Json(body): Json<ConfigureRequest>,
) -> Result<Json<ConfigureResponse>, (axum::http::StatusCode, Json<ConfigureResponse>)> {
    if state.cloud_mode {
        return Err((
            axum::http::StatusCode::FORBIDDEN,
            Json(ConfigureResponse {
                success: false,
                message: "local key management is not available in cloud mode".to_string(),
            }),
        ));
    }

    // Require BEACON_API_KEY if one is configured
    if let Some(ref expected_key) = state.api_key {
        let provided = headers
            .get("x-api-key")
            .or_else(|| headers.get("authorization"))
            .and_then(|v| v.to_str().ok())
            .map(|v| v.strip_prefix("Bearer ").unwrap_or(v));
        if provided != Some(expected_key.as_str()) {
            return Err((
                axum::http::StatusCode::UNAUTHORIZED,
                Json(ConfigureResponse {
                    success: false,
                    message: "invalid or missing API key".to_string(),
                }),
            ));
        }
    }

    let valid_providers = ["anthropic", "openai", "openrouter"];
    if !valid_providers.contains(&body.provider.as_str()) {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            Json(ConfigureResponse {
                success: false,
                message: format!("unknown provider: {}", body.provider),
            }),
        ));
    }

    let Some(store) = &state.local_key_store else {
        return Err((
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Json(ConfigureResponse {
                success: false,
                message: "local key store not available".to_string(),
            }),
        ));
    };

    match store.set(
        &body.provider,
        &body.api_key,
        body.model_preference.as_deref(),
    ) {
        Ok(()) => {
            tracing::info!(provider = %body.provider, "local provider key configured");
            Ok(Json(ConfigureResponse {
                success: true,
                message: format!("{} configured successfully", body.provider),
            }))
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to save local provider key");
            Err((
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(ConfigureResponse {
                    success: false,
                    message: "failed to save provider key".to_string(),
                }),
            ))
        }
    }
}

/// Remove a locally configured provider key
#[allow(clippy::similar_names)]
async fn remove_provider(
    headers: HeaderMap,
    State(state): State<Arc<ApiState>>,
    axum::extract::Path(provider): axum::extract::Path<String>,
) -> Result<Json<ConfigureResponse>, (axum::http::StatusCode, Json<ConfigureResponse>)> {
    if state.cloud_mode {
        return Err((
            axum::http::StatusCode::FORBIDDEN,
            Json(ConfigureResponse {
                success: false,
                message: "local key management is not available in cloud mode".to_string(),
            }),
        ));
    }

    // Require BEACON_API_KEY if one is configured
    if let Some(ref expected_key) = state.api_key {
        let header_key = headers
            .get("x-api-key")
            .or_else(|| headers.get("authorization"))
            .and_then(|v| v.to_str().ok())
            .map(|v| v.strip_prefix("Bearer ").unwrap_or(v));
        if header_key != Some(expected_key.as_str()) {
            return Err((
                axum::http::StatusCode::UNAUTHORIZED,
                Json(ConfigureResponse {
                    success: false,
                    message: "invalid or missing API key".to_string(),
                }),
            ));
        }
    }

    let Some(store) = &state.local_key_store else {
        return Err((
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Json(ConfigureResponse {
                success: false,
                message: "local key store not available".to_string(),
            }),
        ));
    };

    match store.remove(&provider) {
        Ok(()) => Ok(Json(ConfigureResponse {
            success: true,
            message: format!("{provider} key removed"),
        })),
        Err(e) => {
            tracing::error!(error = %e, "failed to remove local provider key");
            Err((
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(ConfigureResponse {
                    success: false,
                    message: "failed to remove provider key".to_string(),
                }),
            ))
        }
    }
}

/// Create the providers router
pub fn router(state: Arc<ApiState>) -> Router {
    Router::new()
        .route("/", get(list_providers))
        .route("/models", get(list_models))
        .route("/configure", post(configure_provider))
        .route("/{provider}", delete(remove_provider))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_type_serialization() {
        let provider = ProviderType::Openai;
        let json = serde_json::to_string(&provider).unwrap();
        assert_eq!(json, "\"openai\"");

        let provider = ProviderType::OmniCredits;
        let json = serde_json::to_string(&provider).unwrap();
        assert_eq!(json, "\"omni_credits\"");
    }
}
