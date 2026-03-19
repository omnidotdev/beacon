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
    pub fn get_session_id(
        &self,
        agent_id: &str,
        channel: &str,
        channel_id: &str,
    ) -> Result<Option<String>> {
        let conn = self
            .pool
            .get()
            .map_err(|e| crate::Error::Database(e.to_string()))?;
        let mut stmt = conn.prepare_cached(
            "SELECT harness_session_id FROM harness_sessions
             WHERE agent_id = ?1 AND channel = ?2 AND channel_id = ?3",
        )?;
        let result = stmt
            .query_row(rusqlite::params![agent_id, channel, channel_id], |row| {
                row.get(0)
            })
            .ok();
        Ok(result)
    }

    /// Store or update the session ID for an agent+channel combo
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn upsert_session_id(
        &self,
        agent_id: &str,
        channel: &str,
        channel_id: &str,
        harness_session_id: &str,
        adapter: &str,
    ) -> Result<()> {
        let conn = self
            .pool
            .get()
            .map_err(|e| crate::Error::Database(e.to_string()))?;
        conn.execute(
            "INSERT INTO harness_sessions (id, agent_id, channel, channel_id, harness_session_id, adapter)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(agent_id, channel, channel_id)
             DO UPDATE SET harness_session_id = ?5, adapter = ?6, updated_at = datetime('now')",
            rusqlite::params![
                uuid::Uuid::new_v4().to_string(),
                agent_id,
                channel,
                channel_id,
                harness_session_id,
                adapter,
            ],
        )?;
        Ok(())
    }

    /// Delete a stored session (e.g. when session is stale)
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub fn delete_session(&self, agent_id: &str, channel: &str, channel_id: &str) -> Result<()> {
        let conn = self
            .pool
            .get()
            .map_err(|e| crate::Error::Database(e.to_string()))?;
        conn.execute(
            "DELETE FROM harness_sessions WHERE agent_id = ?1 AND channel = ?2 AND channel_id = ?3",
            rusqlite::params![agent_id, channel, channel_id],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_pool() -> DbPool {
        crate::db::init_memory().unwrap()
    }

    #[test]
    fn get_returns_none_when_empty() {
        let repo = HarnessSessionRepo::new(test_pool());
        let result = repo.get_session_id("agent1", "discord", "chan1").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn upsert_and_get_roundtrip() {
        let repo = HarnessSessionRepo::new(test_pool());
        repo.upsert_session_id("agent1", "discord", "chan1", "sess_abc", "claude_cli")
            .unwrap();
        let result = repo
            .get_session_id("agent1", "discord", "chan1")
            .unwrap()
            .unwrap();
        assert_eq!(result, "sess_abc");
    }

    #[test]
    fn upsert_overwrites_existing() {
        let repo = HarnessSessionRepo::new(test_pool());
        repo.upsert_session_id("agent1", "discord", "chan1", "sess_old", "claude_cli")
            .unwrap();
        repo.upsert_session_id("agent1", "discord", "chan1", "sess_new", "claude_cli")
            .unwrap();
        let result = repo
            .get_session_id("agent1", "discord", "chan1")
            .unwrap()
            .unwrap();
        assert_eq!(result, "sess_new");
    }

    #[test]
    fn delete_removes_session() {
        let repo = HarnessSessionRepo::new(test_pool());
        repo.upsert_session_id("agent1", "discord", "chan1", "sess_abc", "claude_cli")
            .unwrap();
        repo.delete_session("agent1", "discord", "chan1").unwrap();
        let result = repo.get_session_id("agent1", "discord", "chan1").unwrap();
        assert!(result.is_none());
    }
}
