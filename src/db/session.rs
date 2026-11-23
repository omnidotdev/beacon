//! Session repository for CRUD operations

use chrono::{DateTime, Utc};
use uuid::Uuid;

use sqlx::Row;
use sqlx::any::AnyRow;

use super::DbPool;
use crate::{Error, Result};

/// A conversation session
#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub user_id: String,
    pub channel: String,
    pub channel_id: String,
    pub persona_id: String,
    pub agent_id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A message in a session
#[derive(Debug, Clone)]
pub struct Message {
    pub id: String,
    pub session_id: String,
    pub role: MessageRole,
    pub content: String,
    pub created_at: DateTime<Utc>,
    /// Thread identifier for grouping related messages
    pub thread_id: Option<String>,
}

/// Message role
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageRole {
    User,
    Assistant,
    System,
}

impl MessageRole {
    const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::System => "system",
        }
    }

    /// Display-friendly label for compaction summaries
    #[must_use]
    pub const fn as_display_str(self) -> &'static str {
        match self {
            Self::User => "User",
            Self::Assistant => "Assistant",
            Self::System => "System",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "user" => Some(Self::User),
            "assistant" => Some(Self::Assistant),
            "system" => Some(Self::System),
            _ => None,
        }
    }
}

const SESSION_COLUMNS: &str =
    "id, user_id, channel, channel_id, persona_id, agent_id, created_at, updated_at";
const MESSAGE_COLUMNS: &str = "id, session_id, role, content, created_at, thread_id";

impl Session {
    fn from_row(row: &AnyRow) -> Result<Self> {
        Ok(Self {
            id: row.try_get(0)?,
            user_id: row.try_get(1)?,
            channel: row.try_get(2)?,
            channel_id: row.try_get(3)?,
            persona_id: row.try_get(4)?,
            agent_id: row.try_get(5)?,
            created_at: parse_datetime(&row.try_get::<String, _>(6)?),
            updated_at: parse_datetime(&row.try_get::<String, _>(7)?),
        })
    }
}

impl Message {
    fn from_row(row: &AnyRow) -> Result<Self> {
        Ok(Self {
            id: row.try_get(0)?,
            session_id: row.try_get(1)?,
            role: MessageRole::from_str(&row.try_get::<String, _>(2)?).unwrap_or(MessageRole::User),
            content: row.try_get(3)?,
            created_at: parse_datetime(&row.try_get::<String, _>(4)?),
            thread_id: row.try_get(5)?,
        })
    }
}

/// Limit as a 64-bit bind (`LIMIT` takes an integer on every backend)
#[allow(clippy::cast_possible_wrap)]
const fn limit_param(limit: usize) -> i64 {
    limit as i64
}

/// Session repository
#[derive(Clone)]
pub struct SessionRepo {
    pool: DbPool,
}

impl SessionRepo {
    /// Create a new session repository
    #[must_use]
    #[allow(clippy::missing_const_for_fn)]
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Find or create a session for a channel conversation
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn find_or_create(
        &self,
        user_id: &str,
        channel: &str,
        channel_id: &str,
        persona_id: &str,
    ) -> Result<Session> {
        self.find_or_create_with_agent(user_id, channel, channel_id, persona_id, "default")
            .await
    }

    /// Find or create a session for a channel conversation with agent routing
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn find_or_create_with_agent(
        &self,
        user_id: &str,
        channel: &str,
        channel_id: &str,
        persona_id: &str,
        agent_id: &str,
    ) -> Result<Session> {
        // Try to find existing session for this channel + agent
        let existing = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {SESSION_COLUMNS} FROM sessions
             WHERE channel = $1 AND channel_id = $2 AND agent_id = $3"
        )))
        .bind(channel.to_string())
        .bind(channel_id.to_string())
        .bind(agent_id.to_string())
        .fetch_optional(self.pool.pool())
        .await?;

        if let Some(row) = existing {
            return Session::from_row(&row);
        }

        // Create new session
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            "INSERT INTO sessions (id, user_id, channel, channel_id, persona_id, agent_id, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $7)",
        )
        .bind(id.clone())
        .bind(user_id.to_string())
        .bind(channel.to_string())
        .bind(channel_id.to_string())
        .bind(persona_id.to_string())
        .bind(agent_id.to_string())
        .bind(now)
        .execute(self.pool.pool())
        .await?;

        Ok(Session {
            id,
            user_id: user_id.to_string(),
            channel: channel.to_string(),
            channel_id: channel_id.to_string(),
            persona_id: persona_id.to_string(),
            agent_id: agent_id.to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
    }

    /// List all sessions
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn list_all(&self) -> Result<Vec<Session>> {
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {SESSION_COLUMNS} FROM sessions ORDER BY updated_at DESC"
        )))
        .fetch_all(self.pool.pool())
        .await?;

        Ok(rows
            .iter()
            .filter_map(|row| Session::from_row(row).ok())
            .collect())
    }

    /// Add a message to a session
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn add_message(
        &self,
        session_id: &str,
        role: MessageRole,
        content: &str,
    ) -> Result<Message> {
        self.add_message_with_thread(session_id, role, content, None)
            .await
    }

    /// Add a message to a session with thread context
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn add_message_with_thread(
        &self,
        session_id: &str,
        role: MessageRole,
        content: &str,
        thread_id: Option<&str>,
    ) -> Result<Message> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        let now_str = now.to_rfc3339();

        let mut tx = self.pool.pool().begin().await?;

        sqlx::query(
            "INSERT INTO messages (id, session_id, role, content, created_at, thread_id)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(id.clone())
        .bind(session_id.to_string())
        .bind(role.as_str())
        .bind(content.to_string())
        .bind(now_str.clone())
        .bind(thread_id.map(String::from))
        .execute(&mut *tx)
        .await?;

        // Update session updated_at
        sqlx::query("UPDATE sessions SET updated_at = $1 WHERE id = $2")
            .bind(now_str)
            .bind(session_id.to_string())
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;

        Ok(Message {
            id,
            session_id: session_id.to_string(),
            role,
            content: content.to_string(),
            created_at: now,
            thread_id: thread_id.map(String::from),
        })
    }

    /// Get recent messages for a session
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn get_messages(&self, session_id: &str, limit: usize) -> Result<Vec<Message>> {
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {MESSAGE_COLUMNS} FROM messages WHERE session_id = $1
             ORDER BY created_at DESC LIMIT $2"
        )))
        .bind(session_id.to_string())
        .bind(limit_param(limit))
        .fetch_all(self.pool.pool())
        .await?;

        Ok(rows
            .iter()
            .filter_map(|row| Message::from_row(row).ok())
            .rev()
            .collect())
    }

    /// Get recent messages for a specific thread within a session
    ///
    /// If `thread_id` is None, returns messages that have no thread (root-level messages)
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn get_messages_in_thread(
        &self,
        session_id: &str,
        thread_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Message>> {
        let rows = if let Some(tid) = thread_id {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "SELECT {MESSAGE_COLUMNS} FROM messages WHERE session_id = $1 AND thread_id = $2
                 ORDER BY created_at DESC LIMIT $3"
            )))
            .bind(session_id.to_string())
            .bind(tid.to_string())
            .bind(limit_param(limit))
            .fetch_all(self.pool.pool())
            .await?
        } else {
            // Get messages with no thread (root level)
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "SELECT {MESSAGE_COLUMNS} FROM messages WHERE session_id = $1 AND thread_id IS NULL
                 ORDER BY created_at DESC LIMIT $2"
            )))
            .bind(session_id.to_string())
            .bind(limit_param(limit))
            .fetch_all(self.pool.pool())
            .await?
        };

        // Reverse to get chronological order
        Ok(rows
            .iter()
            .filter_map(|row| Message::from_row(row).ok())
            .rev()
            .collect())
    }

    /// Delete messages in a session older than the given message ID
    ///
    /// Uses `created_at` ordering to determine which messages are "before" the cutoff.
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn delete_messages_before(&self, session_id: &str, before_id: &str) -> Result<usize> {
        // Find the created_at of the cutoff message
        let cutoff: String =
            sqlx::query_scalar("SELECT created_at FROM messages WHERE id = $1 AND session_id = $2")
                .bind(before_id.to_string())
                .bind(session_id.to_string())
                .fetch_optional(self.pool.pool())
                .await?
                .ok_or_else(|| Error::Database("cutoff message not found".to_string()))?;

        let deleted = sqlx::query("DELETE FROM messages WHERE session_id = $1 AND created_at < $2")
            .bind(session_id.to_string())
            .bind(cutoff)
            .execute(self.pool.pool())
            .await?
            .rows_affected();

        Ok(usize::try_from(deleted).unwrap_or(usize::MAX))
    }

    /// Insert a compaction summary as a system message at the start of a session
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn insert_summary(&self, session_id: &str, summary: &str) -> Result<Message> {
        // Find the earliest message timestamp in the session
        let earliest: Option<String> =
            sqlx::query_scalar("SELECT MIN(created_at) FROM messages WHERE session_id = $1")
                .bind(session_id.to_string())
                .fetch_one(self.pool.pool())
                .await?;

        // Set summary timestamp slightly before the earliest remaining message
        let summary_time = earliest
            .as_deref()
            .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
            .map_or_else(
                || Utc::now().to_rfc3339(),
                |dt| (dt.with_timezone(&Utc) - chrono::Duration::seconds(1)).to_rfc3339(),
            );

        let id = Uuid::new_v4().to_string();

        sqlx::query(
            "INSERT INTO messages (id, session_id, role, content, created_at)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(id.clone())
        .bind(session_id.to_string())
        .bind("system")
        .bind(summary.to_string())
        .bind(summary_time.clone())
        .execute(self.pool.pool())
        .await?;

        Ok(Message {
            id,
            session_id: session_id.to_string(),
            role: MessageRole::System,
            content: summary.to_string(),
            created_at: DateTime::parse_from_rfc3339(&summary_time)
                .map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc)),
            thread_id: None,
        })
    }

    /// Delete a session and all its messages
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn delete_session(&self, session_id: &str) -> Result<()> {
        let mut tx = self.pool.pool().begin().await?;

        sqlx::query("DELETE FROM messages WHERE session_id = $1")
            .bind(session_id.to_string())
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM sessions WHERE id = $1")
            .bind(session_id.to_string())
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(())
    }

    /// Delete sessions older than the given number of days
    ///
    /// Returns the number of sessions deleted
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn prune_older_than(&self, days: i64) -> Result<usize> {
        let cutoff = (Utc::now() - chrono::Duration::days(days)).to_rfc3339();
        let mut tx = self.pool.pool().begin().await?;

        // Delete messages first (foreign key)
        sqlx::query(
            "DELETE FROM messages WHERE session_id IN (SELECT id FROM sessions WHERE updated_at < $1)",
        )
        .bind(cutoff.clone())
        .execute(&mut *tx)
        .await?;

        let deleted = sqlx::query("DELETE FROM sessions WHERE updated_at < $1")
            .bind(cutoff)
            .execute(&mut *tx)
            .await?
            .rows_affected();

        tx.commit().await?;
        Ok(usize::try_from(deleted).unwrap_or(usize::MAX))
    }

    /// Count total sessions
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn count(&self) -> Result<usize> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
            .fetch_one(self.pool.pool())
            .await?;

        Ok(usize::try_from(count).unwrap_or(0))
    }

    /// Get total disk usage of all messages (approximate, in bytes)
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn disk_usage(&self) -> Result<usize> {
        let total: i64 = sqlx::query_scalar(
            "SELECT CAST(COALESCE(SUM(LENGTH(content)), 0) AS BIGINT) FROM messages",
        )
        .fetch_one(self.pool.pool())
        .await?;

        Ok(usize::try_from(total).unwrap_or(0))
    }

    /// Count messages in a session
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn message_count(&self, session_id: &str) -> Result<usize> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE session_id = $1")
            .bind(session_id.to_string())
            .fetch_one(self.pool.pool())
            .await?;

        Ok(usize::try_from(count).unwrap_or(0))
    }
}

fn parse_datetime(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_memory;

    async fn setup() -> SessionRepo {
        let pool = init_memory().await.unwrap();

        // Create a test user
        sqlx::query("INSERT INTO users (id) VALUES ('test-user')")
            .execute(pool.pool())
            .await
            .unwrap();

        SessionRepo::new(pool)
    }

    #[tokio::test]
    async fn test_find_or_create_session() {
        let repo = setup().await;

        let session = repo
            .find_or_create("test-user", "discord", "channel-123", "orin")
            .await
            .unwrap();

        assert_eq!(session.channel, "discord");
        assert_eq!(session.channel_id, "channel-123");

        // Should return same session
        let session2 = repo
            .find_or_create("test-user", "discord", "channel-123", "orin")
            .await
            .unwrap();

        assert_eq!(session.id, session2.id);
    }

    #[tokio::test]
    async fn test_add_and_get_messages() {
        let repo = setup().await;

        let session = repo
            .find_or_create("test-user", "voice", "local", "orin")
            .await
            .unwrap();

        repo.add_message(&session.id, MessageRole::User, "Hello")
            .await
            .unwrap();
        repo.add_message(&session.id, MessageRole::Assistant, "Hi there!")
            .await
            .unwrap();

        let messages = repo.get_messages(&session.id, 10).await.unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].content, "Hello");
        assert_eq!(messages[1].content, "Hi there!");
        assert!(messages[0].thread_id.is_none());
    }

    #[tokio::test]
    async fn test_thread_messages() {
        let repo = setup().await;

        let session = repo
            .find_or_create("test-user", "discord", "channel-123", "orin")
            .await
            .unwrap();

        // Add root-level message
        repo.add_message(&session.id, MessageRole::User, "Root message")
            .await
            .unwrap();

        // Add threaded messages
        repo.add_message_with_thread(
            &session.id,
            MessageRole::User,
            "Thread message 1",
            Some("thread-abc"),
        )
        .await
        .unwrap();
        repo.add_message_with_thread(
            &session.id,
            MessageRole::Assistant,
            "Thread reply 1",
            Some("thread-abc"),
        )
        .await
        .unwrap();

        // Get all messages
        let all = repo.get_messages(&session.id, 10).await.unwrap();
        assert_eq!(all.len(), 3);

        // Get only root-level messages
        let root = repo
            .get_messages_in_thread(&session.id, None, 10)
            .await
            .unwrap();
        assert_eq!(root.len(), 1);
        assert_eq!(root[0].content, "Root message");

        // Get only threaded messages
        let threaded = repo
            .get_messages_in_thread(&session.id, Some("thread-abc"), 10)
            .await
            .unwrap();
        assert_eq!(threaded.len(), 2);
        assert_eq!(threaded[0].content, "Thread message 1");
        assert_eq!(threaded[1].content, "Thread reply 1");
    }

    #[tokio::test]
    async fn test_message_count() {
        let repo = setup().await;

        let session = repo
            .find_or_create("test-user", "slack", "channel-456", "orin")
            .await
            .unwrap();

        assert_eq!(repo.message_count(&session.id).await.unwrap(), 0);

        repo.add_message(&session.id, MessageRole::User, "Test")
            .await
            .unwrap();

        assert_eq!(repo.message_count(&session.id).await.unwrap(), 1);
    }
}
