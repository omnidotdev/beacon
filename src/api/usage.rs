//! Usage query API endpoint

use std::sync::Arc;

use axum::{Json, Router, extract::Query, extract::State, routing::get};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use super::ApiState;

/// Query parameters for usage endpoint
#[derive(Debug, Deserialize)]
pub struct UsageQuery {
    /// ISO 8601 start time (defaults to 24h ago)
    pub since: Option<String>,
    /// Model filter
    pub model: Option<String>,
}

/// Usage summary response
#[derive(Debug, Serialize)]
pub struct UsageSummary {
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_estimated_cost_usd: f64,
    pub record_count: usize,
    pub records: Vec<UsageRecordResponse>,
}

/// Individual usage record in response
#[derive(Debug, Serialize)]
pub struct UsageRecordResponse {
    pub model: String,
    pub provider: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub estimated_cost_usd: f64,
    pub created_at: String,
}

/// Build usage API router
pub fn router(state: Arc<ApiState>) -> Router {
    Router::new()
        .route("/usage", get(get_usage))
        .with_state(state)
}

/// Handle GET /api/usage
async fn get_usage(
    State(state): State<Arc<ApiState>>,
    Query(query): Query<UsageQuery>,
) -> Json<UsageSummary> {
    let since = query
        .since
        .as_deref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map_or_else(
            || Utc::now() - Duration::hours(24),
            |dt| dt.with_timezone(&Utc),
        );

    let records = state
        .usage_repo
        .as_ref()
        .map(|repo| {
            repo.query_by_time_range(&since, None, query.model.as_deref())
                .unwrap_or_default()
        })
        .unwrap_or_default();

    let total_input_tokens: u64 = records.iter().map(|r| u64::from(r.input_tokens)).sum();
    let total_output_tokens: u64 = records.iter().map(|r| u64::from(r.output_tokens)).sum();
    let total_estimated_cost_usd: f64 = records.iter().map(|r| r.estimated_cost_usd).sum();
    let record_count = records.len();

    let response_records: Vec<UsageRecordResponse> = records
        .into_iter()
        .map(|r| UsageRecordResponse {
            model: r.model,
            provider: r.provider,
            input_tokens: r.input_tokens,
            output_tokens: r.output_tokens,
            estimated_cost_usd: r.estimated_cost_usd,
            created_at: r.created_at.to_rfc3339(),
        })
        .collect();

    Json(UsageSummary {
        total_input_tokens,
        total_output_tokens,
        total_estimated_cost_usd,
        record_count,
        records: response_records,
    })
}
