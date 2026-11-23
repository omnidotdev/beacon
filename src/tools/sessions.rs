//! Inter-session tools for agent communication
//!
//! Provides tools for listing, inspecting, and communicating between sessions

use std::fmt;

use serde::Serialize;

use crate::Result;
use crate::db::{MessageRole, SessionRepo};

/// Tools for inter-session communication
#[derive(Clone)]
pub struct SessionTools {
    session_repo: SessionRepo,
}

impl fmt::Debug for SessionTools {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionTools").finish_non_exhaustive()
    }
}

/// Summary information about a session
#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    /// Unique session identifier
    pub id: String,
    /// Channel type (e.g., "voice", "discord", "slack")
    pub channel: String,
    /// Channel-specific identifier
    pub channel_id: String,
    /// User ID associated with the session
    pub user_id: String,
    /// Number of messages in the session
    pub message_count: usize,
    /// Last update time (ISO 8601 string)
    pub updated_at: String,
}

/// Message information for history retrieval
#[derive(Debug, Clone, Serialize)]
pub struct MessageInfo {
    /// Message role (user, assistant, system)
    pub role: String,
    /// Message content
    pub content: String,
    /// Creation time (ISO 8601 string)
    pub created_at: String,
}

impl SessionTools {
    /// Create a new `SessionTools` instance
    #[must_use]
    pub const fn new(session_repo: SessionRepo) -> Self {
        Self { session_repo }
    }

    /// List all active sessions
    ///
    /// # Errors
    ///
    /// Returns an error if the database query fails
    pub async fn list(&self) -> Result<Vec<SessionInfo>> {
        let sessions = self.session_repo.list_all().await?;

        let mut infos = Vec::with_capacity(sessions.len());
        for s in sessions {
            let message_count = self.session_repo.message_count(&s.id).await.unwrap_or(0);
            infos.push(SessionInfo {
                id: s.id,
                channel: s.channel,
                channel_id: s.channel_id,
                user_id: s.user_id,
                message_count,
                updated_at: s.updated_at.to_rfc3339(),
            });
        }

        Ok(infos)
    }

    /// Get message history from a session
    ///
    /// # Arguments
    ///
    /// * `session_id` - ID of the session to retrieve messages from
    /// * `limit` - Maximum number of messages to retrieve
    ///
    /// # Errors
    ///
    /// Returns an error if the database query fails
    pub async fn history(&self, session_id: &str, limit: usize) -> Result<Vec<MessageInfo>> {
        let messages = self.session_repo.get_messages(session_id, limit).await?;

        let infos = messages
            .into_iter()
            .map(|m| MessageInfo {
                role: match m.role {
                    MessageRole::User => "user".to_string(),
                    MessageRole::Assistant => "assistant".to_string(),
                    MessageRole::System => "system".to_string(),
                },
                content: m.content,
                created_at: m.created_at.to_rfc3339(),
            })
            .collect();

        Ok(infos)
    }

    /// Send a message to another session
    ///
    /// This stores a system message that will be seen in that session's context.
    /// Useful for cross-session communication between agent instances.
    ///
    /// # Arguments
    ///
    /// * `session_id` - ID of the target session
    /// * `content` - Message content to send
    ///
    /// # Errors
    ///
    /// Returns an error if the database operation fails
    pub async fn send(&self, session_id: &str, content: &str) -> Result<()> {
        self.session_repo
            .add_message(session_id, MessageRole::System, content)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_memory;

    async fn setup() -> SessionTools {
        let pool = init_memory().await.unwrap();

        // Create a test user
        sqlx::query("INSERT INTO users (id) VALUES ('test-user')")
            .execute(pool.pool())
            .await
            .unwrap();

        let session_repo = SessionRepo::new(pool);
        SessionTools::new(session_repo)
    }

    #[tokio::test]
    async fn test_list_empty() {
        let tools = setup().await;
        let sessions = tools.list().await.unwrap();
        assert!(sessions.is_empty());
    }

    #[tokio::test]
    async fn test_list_with_sessions() {
        let tools = setup().await;

        // Create some sessions
        tools
            .session_repo
            .find_or_create("test-user", "voice", "local", "orin")
            .await
            .unwrap();
        tools
            .session_repo
            .find_or_create("test-user", "discord", "channel-123", "orin")
            .await
            .unwrap();

        let sessions = tools.list().await.unwrap();
        assert_eq!(sessions.len(), 2);
    }

    #[tokio::test]
    async fn test_history() {
        let tools = setup().await;

        let session = tools
            .session_repo
            .find_or_create("test-user", "voice", "local", "orin")
            .await
            .unwrap();

        tools
            .session_repo
            .add_message(&session.id, MessageRole::User, "Hello")
            .await
            .unwrap();
        tools
            .session_repo
            .add_message(&session.id, MessageRole::Assistant, "Hi there!")
            .await
            .unwrap();

        let history = tools.history(&session.id, 10).await.unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].role, "user");
        assert_eq!(history[0].content, "Hello");
        assert_eq!(history[1].role, "assistant");
        assert_eq!(history[1].content, "Hi there!");
    }

    #[tokio::test]
    async fn test_send() {
        let tools = setup().await;

        let session = tools
            .session_repo
            .find_or_create("test-user", "voice", "local", "orin")
            .await
            .unwrap();

        tools
            .send(&session.id, "Cross-session message from another agent")
            .await
            .unwrap();

        let history = tools.history(&session.id, 10).await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].role, "system");
        assert_eq!(
            history[0].content,
            "Cross-session message from another agent"
        );
    }

    #[test]
    fn test_session_info_serialization() {
        let info = SessionInfo {
            id: "session-123".to_string(),
            channel: "discord".to_string(),
            channel_id: "channel-456".to_string(),
            user_id: "user-789".to_string(),
            message_count: 42,
            updated_at: "2024-01-15T10:00:00Z".to_string(),
        };

        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("session-123"));
        assert!(json.contains("discord"));
        assert!(json.contains("42"));
    }
}
