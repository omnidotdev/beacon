//! Plan API: the plan and credit balance of the workspace a user's Beacon
//! usage bills (their personal workspace), so the app shows exactly what the
//! gateway enforces

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use super::ApiState;
use crate::billing::tier::tier_name;

/// Plan summary for the signed-in user
#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlanResponse {
    /// Entity the plan belongs to (`organization` for a workspace, `user`
    /// before the personal workspace is known)
    pub entity_type: &'static str,
    /// Workspace id when the plan belongs to a workspace
    pub workspace_id: Option<String>,
    /// Plan tier name (`free`, `pro`, `team`), `None` when unknown
    pub tier: Option<String>,
    /// Credit balance (retail USD), `None` when it could not be read
    pub credit_balance: Option<f64>,
}

/// Build the plan API router
pub fn router(state: Arc<ApiState>) -> Router {
    Router::new()
        .route("/plan", get(get_plan))
        .with_state(state)
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
}

/// Handle GET /api/billing/plan
async fn get_plan(headers: HeaderMap, State(state): State<Arc<ApiState>>) -> Response {
    let (Some(billing), Some(jwt_cache)) = (&state.billing_state, &state.jwt_cache) else {
        return (StatusCode::NOT_FOUND, "billing is not enabled").into_response();
    };

    let Some(token) = bearer(&headers) else {
        return (StatusCode::UNAUTHORIZED, "sign in to see your plan").into_response();
    };

    let Ok(claims) = jwt_cache.validate(token).await else {
        return (StatusCode::UNAUTHORIZED, "sign in to see your plan").into_response();
    };

    let entity = billing.workspaces.resolve(&claims.sub, token).await;

    let (entitlements, credit_balance) = tokio::join!(
        billing
            .client
            .get_entitlements(entity.kind(), &entity.entity_id),
        billing.credit_balance(&entity),
    );

    let tier = match entitlements {
        Ok(entitlements) => tier_name(&entitlements),
        Err(e) => {
            tracing::warn!(error = %e, "plan lookup failed");
            None
        }
    };

    let credit_balance = credit_balance.unwrap_or_else(|e| {
        tracing::warn!(error = %e, "credit balance lookup failed");
        None
    });

    let workspace_id = (entity.entity_type
        == crate::providers::provisioner::BillingEntityType::Organization)
        .then(|| entity.entity_id.clone());

    Json(PlanResponse {
        entity_type: entity.kind(),
        workspace_id,
        tier,
        credit_balance,
    })
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_response_is_camel_case() {
        let body = serde_json::to_value(PlanResponse {
            entity_type: "organization",
            workspace_id: Some("ws-1".to_string()),
            tier: Some("pro".to_string()),
            credit_balance: Some(3.0),
        })
        .unwrap();
        assert_eq!(
            body,
            serde_json::json!({
                "entityType": "organization",
                "workspaceId": "ws-1",
                "tier": "pro",
                "creditBalance": 3.0
            })
        );
    }

    #[test]
    fn reads_only_bearer_tokens() {
        let mut headers = HeaderMap::new();
        assert_eq!(bearer(&headers), None);
        headers.insert(header::AUTHORIZATION, "Basic abc".parse().unwrap());
        assert_eq!(bearer(&headers), None);
        headers.insert(header::AUTHORIZATION, "Bearer abc".parse().unwrap());
        assert_eq!(bearer(&headers), Some("abc"));
    }
}
