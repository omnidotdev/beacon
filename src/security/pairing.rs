//! DM pairing and allowlist security
//!
//! Provides four modes of access control for incoming DMs:
//! - Open: Accept all DMs (no security)
//! - Pairing: New senders must enter a pairing code to be approved
//! - Allowlist: Only pre-approved sender IDs can message
//! - Disabled: Ignore all DMs entirely

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::db::DbPool;
use crate::{Error, Result};

/// Pairing code length
const PAIRING_CODE_LENGTH: usize = 6;

/// Pairing code valid duration in minutes
const PAIRING_CODE_EXPIRY_MINUTES: i64 = 10;

/// DM access policy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DmPolicy {
    /// Accept all DMs without authentication
    #[default]
    Open,

    /// New senders must enter a pairing code
    Pairing,

    /// Only pre-approved sender IDs can message
    Allowlist,

    /// Ignore all DMs entirely
    Disabled,
}

impl DmPolicy {
    /// Parse from string representation
    #[must_use]
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "disabled" | "off" | "none" => Self::Disabled,
            "pairing" => Self::Pairing,
            "allowlist" | "whitelist" => Self::Allowlist,
            _ => Self::Open,
        }
    }
}

impl std::fmt::Display for DmPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open => write!(f, "open"),
            Self::Pairing => write!(f, "pairing"),
            Self::Allowlist => write!(f, "allowlist"),
            Self::Disabled => write!(f, "disabled"),
        }
    }
}

/// A paired user record
#[derive(Debug, Clone)]
pub struct PairedUser {
    pub id: String,
    pub sender_id: String,
    pub channel: String,
    pub paired_at: DateTime<Utc>,
    pub pairing_code: Option<String>,
    pub code_expires_at: Option<DateTime<Utc>>,
}

/// Manages DM pairing and access control
#[derive(Clone)]
pub struct PairingManager {
    policy: DmPolicy,
    pool: DbPool,
}

impl PairingManager {
    /// Create a new pairing manager
    #[must_use]
    pub const fn new(policy: DmPolicy, pool: DbPool) -> Self {
        Self { policy, pool }
    }

    /// Get the current policy
    #[must_use]
    pub const fn policy(&self) -> DmPolicy {
        self.policy
    }

    /// Check if a sender is allowed to message
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn is_allowed(&self, sender_id: &str, channel: &str) -> Result<bool> {
        match self.policy {
            DmPolicy::Disabled => Ok(false),
            DmPolicy::Open => Ok(true),
            DmPolicy::Pairing | DmPolicy::Allowlist => self.is_paired(sender_id, channel).await,
        }
    }

    /// Check if sender is paired (has a valid entry without pending code)
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn is_paired(&self, sender_id: &str, channel: &str) -> Result<bool> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM paired_users
             WHERE sender_id = $1 AND channel = $2 AND pairing_code IS NULL",
        )
        .bind(sender_id.to_string())
        .bind(channel.to_string())
        .fetch_one(self.pool.pool())
        .await?;

        Ok(count > 0)
    }

    /// Generate a pairing code for a new sender
    ///
    /// Returns the code if generated, or None if sender is already paired
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn generate_pairing_code(
        &self,
        sender_id: &str,
        channel: &str,
    ) -> Result<Option<String>> {
        // Check if already paired
        if self.is_paired(sender_id, channel).await? {
            return Ok(None);
        }

        // Generate a 6-character alphanumeric code
        let code = generate_code(PAIRING_CODE_LENGTH);
        let now = Utc::now();
        let expires_at = now + chrono::Duration::minutes(PAIRING_CODE_EXPIRY_MINUTES);

        // Create the pending record, or refresh the code on an existing one
        sqlx::query(
            "INSERT INTO paired_users (id, sender_id, channel, paired_at, pairing_code, code_expires_at)
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (sender_id, channel) DO UPDATE SET
                pairing_code = excluded.pairing_code,
                code_expires_at = excluded.code_expires_at",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(sender_id.to_string())
        .bind(channel.to_string())
        .bind(now.to_rfc3339())
        .bind(code.clone())
        .bind(expires_at.to_rfc3339())
        .execute(self.pool.pool())
        .await
        .map_err(|e| Error::Database(e.to_string()))?;

        tracing::debug!(sender_id, channel, "generated pairing code");
        Ok(Some(code))
    }

    /// Verify a pairing code and approve the sender
    ///
    /// Returns true if code is valid and sender is now paired
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn verify_pairing(&self, sender_id: &str, channel: &str, code: &str) -> Result<bool> {
        use sqlx::Row;

        // Find pending pairing with matching code
        let record = sqlx::query(
            "SELECT id, code_expires_at FROM paired_users
             WHERE sender_id = $1 AND channel = $2 AND pairing_code = $3",
        )
        .bind(sender_id.to_string())
        .bind(channel.to_string())
        .bind(code.to_string())
        .fetch_optional(self.pool.pool())
        .await?;

        let Some(record) = record else {
            tracing::debug!(sender_id, channel, "invalid pairing code");
            return Ok(false);
        };
        let id: String = record.try_get(0)?;
        let expires_at_str: Option<String> = record.try_get(1)?;

        // Check expiry
        if let Some(expires_at) = expires_at_str
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            && Utc::now() > expires_at.with_timezone(&Utc)
        {
            tracing::debug!(sender_id, channel, "pairing code expired");
            return Ok(false);
        }

        // Clear the code to mark as paired
        sqlx::query(
            "UPDATE paired_users SET pairing_code = NULL, code_expires_at = NULL, paired_at = $1 WHERE id = $2",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(id)
        .execute(self.pool.pool())
        .await?;

        tracing::info!(sender_id, channel, "sender paired successfully");
        Ok(true)
    }

    /// Add a sender directly to the allowlist (bypassing pairing flow)
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn add_to_allowlist(&self, sender_id: &str, channel: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO paired_users (id, sender_id, channel, paired_at)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (sender_id, channel) DO UPDATE SET pairing_code = NULL, code_expires_at = NULL",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(sender_id.to_string())
        .bind(channel.to_string())
        .bind(Utc::now().to_rfc3339())
        .execute(self.pool.pool())
        .await?;

        tracing::info!(sender_id, channel, "added to allowlist");
        Ok(())
    }

    /// Remove a sender from the allowlist
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn remove_from_allowlist(&self, sender_id: &str, channel: &str) -> Result<()> {
        sqlx::query("DELETE FROM paired_users WHERE sender_id = $1 AND channel = $2")
            .bind(sender_id.to_string())
            .bind(channel.to_string())
            .execute(self.pool.pool())
            .await?;

        tracing::info!(sender_id, channel, "removed from allowlist");
        Ok(())
    }

    /// List all paired users for a channel
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn list_paired(&self, channel: &str) -> Result<Vec<PairedUser>> {
        let rows = sqlx::query(
            "SELECT id, sender_id, channel, paired_at, pairing_code, code_expires_at
             FROM paired_users
             WHERE channel = $1 AND pairing_code IS NULL
             ORDER BY paired_at DESC",
        )
        .bind(channel.to_string())
        .fetch_all(self.pool.pool())
        .await?;

        Ok(rows.iter().filter_map(paired_user_from_row).collect())
    }

    /// List all paired users across all channels
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn list_all_paired(&self) -> Result<Vec<PairedUser>> {
        let rows = sqlx::query(
            "SELECT id, sender_id, channel, paired_at, pairing_code, code_expires_at
             FROM paired_users
             WHERE pairing_code IS NULL
             ORDER BY paired_at DESC",
        )
        .fetch_all(self.pool.pool())
        .await?;

        Ok(rows.iter().filter_map(paired_user_from_row).collect())
    }
}

fn paired_user_from_row(row: &sqlx::any::AnyRow) -> Option<PairedUser> {
    use sqlx::Row;

    Some(PairedUser {
        id: row.try_get(0).ok()?,
        sender_id: row.try_get(1).ok()?,
        channel: row.try_get(2).ok()?,
        paired_at: parse_datetime(&row.try_get::<String, _>(3).ok()?),
        pairing_code: row.try_get(4).ok()?,
        code_expires_at: row
            .try_get::<Option<String>, _>(5)
            .ok()?
            .map(|s| parse_datetime(&s)),
    })
}

fn generate_code(length: usize) -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};

    const CHARSET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut code = String::with_capacity(length);
    let hasher_builder = RandomState::new();

    for i in 0..length {
        let mut hasher = hasher_builder.build_hasher();
        hasher.write_usize(i);
        hasher.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        );
        #[allow(clippy::cast_possible_truncation)]
        let idx = hasher.finish() as usize % CHARSET.len();
        code.push(CHARSET[idx] as char);
    }

    code
}

fn parse_datetime(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_memory;

    async fn setup(policy: DmPolicy) -> PairingManager {
        let pool = init_memory().await.unwrap();
        PairingManager::new(policy, pool)
    }

    #[tokio::test]
    async fn test_open_policy_allows_all() {
        let manager = setup(DmPolicy::Open).await;
        assert!(manager.is_allowed("anyone", "discord").await.unwrap());
    }

    #[tokio::test]
    async fn test_allowlist_denies_unknown() {
        let manager = setup(DmPolicy::Allowlist).await;
        assert!(!manager.is_allowed("unknown", "discord").await.unwrap());
    }

    #[tokio::test]
    async fn test_add_to_allowlist() {
        let manager = setup(DmPolicy::Allowlist).await;

        manager
            .add_to_allowlist("user123", "discord")
            .await
            .unwrap();
        assert!(manager.is_allowed("user123", "discord").await.unwrap());

        // Different channel should not be allowed
        assert!(!manager.is_allowed("user123", "slack").await.unwrap());
    }

    #[tokio::test]
    async fn test_pairing_flow() {
        let manager = setup(DmPolicy::Pairing).await;

        // Initially not allowed
        assert!(!manager.is_allowed("newuser", "telegram").await.unwrap());

        // Generate code
        let code = manager
            .generate_pairing_code("newuser", "telegram")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(code.len(), PAIRING_CODE_LENGTH);

        // Still not allowed with pending code
        assert!(!manager.is_allowed("newuser", "telegram").await.unwrap());

        // Wrong code fails
        assert!(
            !manager
                .verify_pairing("newuser", "telegram", "WRONG1")
                .await
                .unwrap()
        );

        // Correct code succeeds
        assert!(
            manager
                .verify_pairing("newuser", "telegram", &code)
                .await
                .unwrap()
        );

        // Now allowed
        assert!(manager.is_allowed("newuser", "telegram").await.unwrap());

        // Code can't be reused
        assert!(
            !manager
                .verify_pairing("newuser", "telegram", &code)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn test_already_paired_returns_none() {
        let manager = setup(DmPolicy::Pairing).await;

        manager
            .add_to_allowlist("existinguser", "slack")
            .await
            .unwrap();

        // Should return None for already paired user
        let code = manager
            .generate_pairing_code("existinguser", "slack")
            .await
            .unwrap();
        assert!(code.is_none());
    }

    #[tokio::test]
    async fn test_remove_from_allowlist() {
        let manager = setup(DmPolicy::Allowlist).await;

        manager.add_to_allowlist("user456", "signal").await.unwrap();
        assert!(manager.is_allowed("user456", "signal").await.unwrap());

        manager
            .remove_from_allowlist("user456", "signal")
            .await
            .unwrap();
        assert!(!manager.is_allowed("user456", "signal").await.unwrap());
    }

    #[tokio::test]
    async fn test_list_paired() {
        let manager = setup(DmPolicy::Allowlist).await;

        manager.add_to_allowlist("alice", "discord").await.unwrap();
        manager.add_to_allowlist("bob", "discord").await.unwrap();
        manager.add_to_allowlist("charlie", "slack").await.unwrap();

        let discord_users = manager.list_paired("discord").await.unwrap();
        assert_eq!(discord_users.len(), 2);

        let all_users = manager.list_all_paired().await.unwrap();
        assert_eq!(all_users.len(), 3);
    }

    #[tokio::test]
    async fn test_generate_code_format() {
        let code = generate_code(6);
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[tokio::test]
    async fn test_disabled_policy_denies_all() {
        let manager = setup(DmPolicy::Disabled).await;

        // Even after adding to allowlist, disabled policy always denies
        manager
            .add_to_allowlist("user789", "discord")
            .await
            .unwrap();
        assert!(!manager.is_allowed("user789", "discord").await.unwrap());
        assert!(!manager.is_allowed("anyone", "discord").await.unwrap());
    }

    #[tokio::test]
    async fn test_dm_policy_from_str() {
        assert_eq!(DmPolicy::from_str("open"), DmPolicy::Open);
        assert_eq!(DmPolicy::from_str("pairing"), DmPolicy::Pairing);
        assert_eq!(DmPolicy::from_str("allowlist"), DmPolicy::Allowlist);
        assert_eq!(DmPolicy::from_str("whitelist"), DmPolicy::Allowlist);
        assert_eq!(DmPolicy::from_str("disabled"), DmPolicy::Disabled);
        assert_eq!(DmPolicy::from_str("off"), DmPolicy::Disabled);
        assert_eq!(DmPolicy::from_str("none"), DmPolicy::Disabled);
        assert_eq!(DmPolicy::from_str("unknown"), DmPolicy::Open);
    }
}
