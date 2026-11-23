//! Local usage tracking for cost estimation and querying

use chrono::{DateTime, Utc};
use uuid::Uuid;

use sqlx::Row;
use sqlx::any::AnyRow;

use super::DbPool;
use crate::Result;

/// A local usage record for a single LLM invocation
#[derive(Debug, Clone)]
pub struct UsageRecord {
    pub id: String,
    pub session_id: String,
    pub agent_id: String,
    pub model: String,
    pub provider: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub estimated_cost_usd: f64,
    pub created_at: DateTime<Utc>,
}

/// Repository for local usage records
#[derive(Clone)]
pub struct UsageRepo {
    pool: DbPool,
}

const COLUMNS: &str = "id, session_id, agent_id, model, provider, input_tokens, output_tokens, estimated_cost_usd, created_at";

impl UsageRepo {
    /// Create a new usage repository
    #[must_use]
    #[allow(clippy::missing_const_for_fn)]
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Record a usage event
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    #[allow(clippy::too_many_arguments)]
    pub async fn record(
        &self,
        session_id: &str,
        agent_id: &str,
        model: &str,
        provider: &str,
        input_tokens: u32,
        output_tokens: u32,
        estimated_cost_usd: f64,
    ) -> Result<UsageRecord> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();

        sqlx::query(
            "INSERT INTO usage_records (id, session_id, agent_id, model, provider, input_tokens, output_tokens, estimated_cost_usd, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(id.clone())
        .bind(session_id.to_string())
        .bind(agent_id.to_string())
        .bind(model.to_string())
        .bind(provider.to_string())
        .bind(i64::from(input_tokens))
        .bind(i64::from(output_tokens))
        .bind(estimated_cost_usd)
        .bind(now.to_rfc3339())
        .execute(self.pool.pool())
        .await?;

        Ok(UsageRecord {
            id,
            session_id: session_id.to_string(),
            agent_id: agent_id.to_string(),
            model: model.to_string(),
            provider: provider.to_string(),
            input_tokens,
            output_tokens,
            estimated_cost_usd,
            created_at: now,
        })
    }

    /// Query usage records by session
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn query_by_session(&self, session_id: &str) -> Result<Vec<UsageRecord>> {
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM usage_records WHERE session_id = $1 ORDER BY created_at DESC"
        )))
        .bind(session_id.to_string())
        .fetch_all(self.pool.pool())
        .await?;

        Ok(rows
            .iter()
            .filter_map(|row| row_to_usage_record(row).ok())
            .collect())
    }

    /// Query usage records within a time range
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn query_by_time_range(
        &self,
        since: &DateTime<Utc>,
        until: Option<&DateTime<Utc>>,
        model_filter: Option<&str>,
    ) -> Result<Vec<UsageRecord>> {
        let since_str = since.to_rfc3339();
        let until_str = until.map_or_else(|| Utc::now().to_rfc3339(), DateTime::to_rfc3339);

        let rows = if let Some(model) = model_filter {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "SELECT {COLUMNS} FROM usage_records
                 WHERE created_at >= $1 AND created_at <= $2 AND model = $3
                 ORDER BY created_at DESC"
            )))
            .bind(since_str)
            .bind(until_str)
            .bind(model.to_string())
            .fetch_all(self.pool.pool())
            .await?
        } else {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "SELECT {COLUMNS} FROM usage_records
                 WHERE created_at >= $1 AND created_at <= $2
                 ORDER BY created_at DESC"
            )))
            .bind(since_str)
            .bind(until_str)
            .fetch_all(self.pool.pool())
            .await?
        };

        Ok(rows
            .iter()
            .filter_map(|row| row_to_usage_record(row).ok())
            .collect())
    }

    /// Get total estimated cost for all records
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn total_cost(&self) -> Result<f64> {
        Ok(
            sqlx::query_scalar("SELECT COALESCE(SUM(estimated_cost_usd), 0.0) FROM usage_records")
                .fetch_one(self.pool.pool())
                .await?,
        )
    }

    /// Delete records older than the given number of days
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn prune_older_than(&self, days: i64) -> Result<usize> {
        let cutoff = (Utc::now() - chrono::Duration::days(days)).to_rfc3339();

        let deleted = sqlx::query("DELETE FROM usage_records WHERE created_at < $1")
            .bind(cutoff)
            .execute(self.pool.pool())
            .await?
            .rows_affected();

        Ok(usize::try_from(deleted).unwrap_or(usize::MAX))
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn row_to_usage_record(row: &AnyRow) -> Result<UsageRecord> {
    Ok(UsageRecord {
        id: row.try_get(0)?,
        session_id: row.try_get(1)?,
        agent_id: row.try_get(2)?,
        model: row.try_get(3)?,
        provider: row.try_get(4)?,
        input_tokens: row.try_get::<i64, _>(5)? as u32,
        output_tokens: row.try_get::<i64, _>(6)? as u32,
        estimated_cost_usd: row.try_get(7)?,
        created_at: parse_datetime(&row.try_get::<String, _>(8)?),
    })
}

fn parse_datetime(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_memory;

    async fn setup() -> UsageRepo {
        let pool = init_memory().await.unwrap();
        UsageRepo::new(pool)
    }

    #[tokio::test]
    async fn record_and_query() {
        let repo = setup().await;

        repo.record(
            "sess-1",
            "default",
            "claude-sonnet-5-5",
            "anthropic",
            1000,
            500,
            0.0105,
        )
        .await
        .unwrap();
        repo.record("sess-1", "default", "gpt-4o", "openai", 2000, 1000, 0.015)
            .await
            .unwrap();

        let records = repo.query_by_session("sess-1").await.unwrap();
        assert_eq!(records.len(), 2);
    }

    #[tokio::test]
    async fn total_cost_sums() {
        let repo = setup().await;

        repo.record("sess-1", "default", "model-a", "provider", 100, 50, 0.01)
            .await
            .unwrap();
        repo.record("sess-2", "default", "model-b", "provider", 200, 100, 0.02)
            .await
            .unwrap();

        let total = repo.total_cost().await.unwrap();
        assert!((total - 0.03).abs() < 0.001);
    }
}
