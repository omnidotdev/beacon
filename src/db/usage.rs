//! Local usage tracking for cost estimation and querying

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::DbPool;
use crate::{Error, Result};

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
    #[allow(clippy::too_many_arguments, clippy::cast_lossless)]
    pub fn record(
        &self,
        session_id: &str,
        agent_id: &str,
        model: &str,
        provider: &str,
        input_tokens: u32,
        output_tokens: u32,
        estimated_cost_usd: f64,
    ) -> Result<UsageRecord> {
        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;

        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        let now_str = now.to_rfc3339();

        conn.execute(
            "INSERT INTO usage_records (id, session_id, agent_id, model, provider, input_tokens, output_tokens, estimated_cost_usd, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                id,
                session_id,
                agent_id,
                model,
                provider,
                i64::from(input_tokens),
                i64::from(output_tokens),
                estimated_cost_usd,
                now_str,
            ],
        )
        .map_err(|e| Error::Database(e.to_string()))?;

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
    pub fn query_by_session(&self, session_id: &str) -> Result<Vec<UsageRecord>> {
        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;

        let mut stmt = conn
            .prepare(
                "SELECT id, session_id, agent_id, model, provider, input_tokens, output_tokens, estimated_cost_usd, created_at
                 FROM usage_records WHERE session_id = ?1
                 ORDER BY created_at DESC",
            )
            .map_err(|e| Error::Database(e.to_string()))?;

        let records = stmt
            .query_map([session_id], row_to_usage_record)
            .map_err(|e| Error::Database(e.to_string()))?
            .filter_map(std::result::Result::ok)
            .collect();

        Ok(records)
    }

    /// Query usage records within a time range
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn query_by_time_range(
        &self,
        since: &DateTime<Utc>,
        until: Option<&DateTime<Utc>>,
        model_filter: Option<&str>,
    ) -> Result<Vec<UsageRecord>> {
        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;

        let since_str = since.to_rfc3339();
        let until_str = until.map_or_else(|| Utc::now().to_rfc3339(), DateTime::to_rfc3339);

        let records = if let Some(model) = model_filter {
            let mut stmt = conn
                .prepare(
                    "SELECT id, session_id, agent_id, model, provider, input_tokens, output_tokens, estimated_cost_usd, created_at
                     FROM usage_records WHERE created_at >= ?1 AND created_at <= ?2 AND model = ?3
                     ORDER BY created_at DESC",
                )
                .map_err(|e| Error::Database(e.to_string()))?;

            stmt.query_map(
                rusqlite::params![since_str, until_str, model],
                row_to_usage_record,
            )
            .map_err(|e| Error::Database(e.to_string()))?
            .filter_map(std::result::Result::ok)
            .collect()
        } else {
            let mut stmt = conn
                .prepare(
                    "SELECT id, session_id, agent_id, model, provider, input_tokens, output_tokens, estimated_cost_usd, created_at
                     FROM usage_records WHERE created_at >= ?1 AND created_at <= ?2
                     ORDER BY created_at DESC",
                )
                .map_err(|e| Error::Database(e.to_string()))?;

            stmt.query_map(rusqlite::params![since_str, until_str], row_to_usage_record)
                .map_err(|e| Error::Database(e.to_string()))?
                .filter_map(std::result::Result::ok)
                .collect()
        };

        Ok(records)
    }

    /// Get total estimated cost for all records
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn total_cost(&self) -> Result<f64> {
        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;

        let total: f64 = conn
            .query_row(
                "SELECT COALESCE(SUM(estimated_cost_usd), 0.0) FROM usage_records",
                [],
                |row| row.get(0),
            )
            .map_err(|e| Error::Database(e.to_string()))?;

        Ok(total)
    }

    /// Delete records older than the given number of days
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn prune_older_than(&self, days: i64) -> Result<usize> {
        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;

        let cutoff = (Utc::now() - chrono::Duration::days(days)).to_rfc3339();

        let deleted = conn
            .execute("DELETE FROM usage_records WHERE created_at < ?1", [&cutoff])
            .map_err(|e| Error::Database(e.to_string()))?;

        Ok(deleted)
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn row_to_usage_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<UsageRecord> {
    Ok(UsageRecord {
        id: row.get(0)?,
        session_id: row.get(1)?,
        agent_id: row.get(2)?,
        model: row.get(3)?,
        provider: row.get(4)?,
        input_tokens: row.get::<_, i64>(5)? as u32,
        output_tokens: row.get::<_, i64>(6)? as u32,
        estimated_cost_usd: row.get(7)?,
        created_at: parse_datetime(&row.get::<_, String>(8)?),
    })
}

fn parse_datetime(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_memory;

    fn setup() -> UsageRepo {
        let pool = init_memory().unwrap();
        UsageRepo::new(pool)
    }

    #[test]
    fn record_and_query() {
        let repo = setup();

        repo.record(
            "sess-1",
            "default",
            "claude-sonnet-4-20250514",
            "anthropic",
            1000,
            500,
            0.0105,
        )
        .unwrap();
        repo.record("sess-1", "default", "gpt-4o", "openai", 2000, 1000, 0.015)
            .unwrap();

        let records = repo.query_by_session("sess-1").unwrap();
        assert_eq!(records.len(), 2);
    }

    #[test]
    fn total_cost_sums() {
        let repo = setup();

        repo.record("sess-1", "default", "model-a", "provider", 100, 50, 0.01)
            .unwrap();
        repo.record("sess-2", "default", "model-b", "provider", 200, 100, 0.02)
            .unwrap();

        let total = repo.total_cost().unwrap();
        assert!((total - 0.03).abs() < 0.001);
    }
}
