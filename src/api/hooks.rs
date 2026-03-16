//! Hook event emission endpoint

use std::sync::Arc;

use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use serde::{Deserialize, Serialize};

use super::ApiState;
use crate::hooks::{HookAction, HookEvent, HookResult};

/// Request body for emitting a custom hook event
#[derive(Debug, Deserialize)]
pub struct EmitHookRequest {
    /// Event name (e.g. "custom:deploy", "message:received")
    pub event: String,
    /// Optional additional context key-value pairs
    #[serde(default)]
    pub context: Option<serde_json::Value>,
}

/// Response from hook emission
#[derive(Debug, Serialize)]
pub struct EmitHookResponse {
    pub skip_processing: bool,
    pub skip_agent: bool,
    pub reply: Option<String>,
    pub modified_response: Option<String>,
    pub messages: Vec<String>,
}

impl From<HookResult> for EmitHookResponse {
    fn from(r: HookResult) -> Self {
        Self {
            skip_processing: r.skip_processing,
            skip_agent: r.skip_agent,
            reply: r.reply,
            modified_response: r.modified_response,
            messages: r.messages,
        }
    }
}

/// Build the hooks API router
pub fn router(state: Arc<ApiState>) -> Router {
    Router::new()
        .route("/emit", post(emit_hook))
        .with_state(state)
}

/// POST /api/hooks/emit — fire a hook event
async fn emit_hook(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<EmitHookRequest>,
) -> Result<Json<EmitHookResponse>, StatusCode> {
    let Some(hook_manager) = &state.hook_manager else {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    };

    if body.event.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    // Parse into a HookAction (falls through to Custom for unknown events)
    let action = HookAction::from_str(&body.event).ok_or(StatusCode::BAD_REQUEST)?;

    // Build a lifecycle-style event (no message context)
    let mut event = HookEvent::channel_lifecycle(action, "api");

    // Merge caller-provided context
    if let Some(serde_json::Value::Object(map)) = body.context {
        for (k, v) in map {
            let value = match v {
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            };
            event = event.with_context(&k, &value);
        }
    }

    let result = hook_manager.trigger(&event).await;
    Ok(Json(EmitHookResponse::from(result)))
}
