//! Session ID persistence for harness adapters

use crate::Result;
use crate::db::DbPool;

/// Store and retrieve harness session IDs
#[derive(Debug, Clone)]
pub struct HarnessSessionRepo {
    pool: DbPool,
}

impl HarnessSessionRepo {
    /// Create a new harness session repository
    #[must_use]
    pub const fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Get the stored session ID for an agent+channel combo
    ///
    /// # Errors
    ///
    /// Returns error if database query fails
    pub async fn get_session_id(
        &self,
        agent_id: &str,
        channel: &str,
        channel_id: &str,
    ) -> Result<Option<String>> {
        Ok(sqlx::query_scalar(
            "SELECT harness_session_id FROM harness_sessions
             WHERE agent_id = $1 AND channel = $2 AND channel_id = $3",
        )
        .bind(agent_id.to_string())
        .bind(channel.to_string())
        .bind(channel_id.to_string())
        .fetch_optional(self.pool.pool())
        .await?)
    }

    /// Store or update the session ID for an agent+channel combo
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn upsert_session_id(
        &self,
        agent_id: &str,
        channel: &str,
        channel_id: &str,
        harness_session_id: &str,
        adapter: &str,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO harness_sessions (id, agent_id, channel, channel_id, harness_session_id, adapter)
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (agent_id, channel, channel_id)
             DO UPDATE SET harness_session_id = excluded.harness_session_id,
                adapter = excluded.adapter, updated_at = $7",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(agent_id.to_string())
        .bind(channel.to_string())
        .bind(channel_id.to_string())
        .bind(harness_session_id.to_string())
        .bind(adapter.to_string())
        .bind(crate::db::now())
        .execute(self.pool.pool())
        .await?;
        Ok(())
    }

    /// Delete a stored session (e.g. when session is stale)
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn delete_session(
        &self,
        agent_id: &str,
        channel: &str,
        channel_id: &str,
    ) -> Result<()> {
        sqlx::query(
            "DELETE FROM harness_sessions WHERE agent_id = $1 AND channel = $2 AND channel_id = $3",
        )
        .bind(agent_id.to_string())
        .bind(channel.to_string())
        .bind(channel_id.to_string())
        .execute(self.pool.pool())
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_pool() -> DbPool {
        crate::db::init_memory().await.unwrap()
    }

    #[tokio::test]
    async fn get_returns_none_when_empty() {
        let repo = HarnessSessionRepo::new(test_pool().await);
        let result = repo
            .get_session_id("agent1", "discord", "chan1")
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn upsert_and_get_roundtrip() {
        let repo = HarnessSessionRepo::new(test_pool().await);
        repo.upsert_session_id("agent1", "discord", "chan1", "sess_abc", "claude_cli")
            .await
            .unwrap();
        let result = repo
            .get_session_id("agent1", "discord", "chan1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result, "sess_abc");
    }

    #[tokio::test]
    async fn upsert_overwrites_existing() {
        let repo = HarnessSessionRepo::new(test_pool().await);
        repo.upsert_session_id("agent1", "discord", "chan1", "sess_old", "claude_cli")
            .await
            .unwrap();
        repo.upsert_session_id("agent1", "discord", "chan1", "sess_new", "claude_cli")
            .await
            .unwrap();
        let result = repo
            .get_session_id("agent1", "discord", "chan1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result, "sess_new");
    }

    #[tokio::test]
    async fn delete_removes_session() {
        let repo = HarnessSessionRepo::new(test_pool().await);
        repo.upsert_session_id("agent1", "discord", "chan1", "sess_abc", "claude_cli")
            .await
            .unwrap();
        repo.delete_session("agent1", "discord", "chan1")
            .await
            .unwrap();
        let result = repo
            .get_session_id("agent1", "discord", "chan1")
            .await
            .unwrap();
        assert!(result.is_none());
    }
}
