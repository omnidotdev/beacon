//! Per-group Telegram configuration repository

use serde::{Deserialize, Serialize};

use sqlx::Row;
use sqlx::any::AnyRow;

use super::DbPool;
use crate::Result;

/// Per-group Telegram configuration override
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramGroupConfig {
    /// Telegram chat ID
    pub chat_id: String,
    /// Group title (informational, updated on upsert)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_title: Option<String>,
    /// Override for `require_mention_in_groups` (None = use global default)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub require_mention: Option<bool>,
    /// Override for reaction level (None = use global default)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reaction_level: Option<String>,
    /// Override for ack reaction emoji
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ack_reaction: Option<String>,
    /// Override for done reaction emoji
    #[serde(skip_serializing_if = "Option::is_none")]
    pub done_reaction: Option<String>,
    /// Whether the bot is enabled in this group
    pub enabled: bool,
}

const COLUMNS: &str =
    "chat_id, chat_title, require_mention, reaction_level, ack_reaction, done_reaction, enabled";

impl TelegramGroupConfig {
    fn from_row(row: &AnyRow) -> Result<Self> {
        Ok(Self {
            chat_id: row.try_get(0)?,
            chat_title: row.try_get(1)?,
            require_mention: row.try_get::<Option<i64>, _>(2)?.map(|v| v != 0),
            reaction_level: row.try_get(3)?,
            ack_reaction: row.try_get(4)?,
            done_reaction: row.try_get(5)?,
            enabled: row.try_get::<i64, _>(6)? != 0,
        })
    }
}

/// Repository for per-group Telegram configuration
#[derive(Debug, Clone)]
pub struct TelegramGroupConfigRepo {
    pool: DbPool,
}

impl TelegramGroupConfigRepo {
    /// Create a new repository
    #[must_use]
    pub const fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Get configuration for a specific chat
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn get(&self, chat_id: &str) -> Result<Option<TelegramGroupConfig>> {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM telegram_group_config WHERE chat_id = $1"
        )))
        .bind(chat_id.to_string())
        .fetch_optional(self.pool.pool())
        .await?
        .as_ref()
        .map(TelegramGroupConfig::from_row)
        .transpose()
    }

    /// Upsert configuration for a group
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn upsert(&self, config: &TelegramGroupConfig) -> Result<()> {
        sqlx::query(
            r"INSERT INTO telegram_group_config (chat_id, chat_title, require_mention, reaction_level, ack_reaction, done_reaction, enabled, updated_at)
              VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
              ON CONFLICT (chat_id) DO UPDATE SET
                chat_title = excluded.chat_title,
                require_mention = excluded.require_mention,
                reaction_level = excluded.reaction_level,
                ack_reaction = excluded.ack_reaction,
                done_reaction = excluded.done_reaction,
                enabled = excluded.enabled,
                updated_at = excluded.updated_at",
        )
        .bind(config.chat_id.clone())
        .bind(config.chat_title.clone())
        .bind(config.require_mention.map(i64::from))
        .bind(config.reaction_level.clone())
        .bind(config.ack_reaction.clone())
        .bind(config.done_reaction.clone())
        .bind(i64::from(config.enabled))
        .bind(super::now())
        .execute(self.pool.pool())
        .await?;

        Ok(())
    }

    /// Delete configuration for a group
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn delete(&self, chat_id: &str) -> Result<bool> {
        let deleted = sqlx::query("DELETE FROM telegram_group_config WHERE chat_id = $1")
            .bind(chat_id.to_string())
            .execute(self.pool.pool())
            .await?
            .rows_affected();

        Ok(deleted > 0)
    }

    /// List all group configurations
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn list(&self) -> Result<Vec<TelegramGroupConfig>> {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM telegram_group_config
             ORDER BY chat_title NULLS FIRST, chat_id"
        )))
        .fetch_all(self.pool.pool())
        .await?
        .iter()
        .map(TelegramGroupConfig::from_row)
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[tokio::test]
    async fn telegram_group_config_crud() {
        let pool = db::init_memory().await.unwrap();
        let repo = TelegramGroupConfigRepo::new(pool);

        // Create
        let config = TelegramGroupConfig {
            chat_id: "-100123456".to_string(),
            chat_title: Some("Test Group".to_string()),
            require_mention: Some(true),
            reaction_level: Some("full".to_string()),
            ack_reaction: None,
            done_reaction: None,
            enabled: true,
        };
        repo.upsert(&config).await.unwrap();

        // Read
        let fetched = repo.get("-100123456").await.unwrap().unwrap();
        assert_eq!(fetched.chat_title.as_deref(), Some("Test Group"));
        assert_eq!(fetched.require_mention, Some(true));
        assert!(fetched.enabled);

        // Update
        let updated = TelegramGroupConfig {
            require_mention: Some(false),
            ..config
        };
        repo.upsert(&updated).await.unwrap();
        let fetched = repo.get("-100123456").await.unwrap().unwrap();
        assert_eq!(fetched.require_mention, Some(false));

        // List
        let all = repo.list().await.unwrap();
        assert_eq!(all.len(), 1);

        // Delete
        assert!(repo.delete("-100123456").await.unwrap());
        assert!(repo.get("-100123456").await.unwrap().is_none());

        // Delete non-existent
        assert!(!repo.delete("-100123456").await.unwrap());
    }
}
