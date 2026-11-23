//! Paired device management
//!
//! Manages devices that have been paired with this gateway instance

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use sqlx::Row;
use sqlx::any::AnyRow;

use crate::db::DbPool;
use crate::{Error, Result};

/// Trust level for paired devices
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustLevel {
    /// Standard paired device
    #[default]
    Paired,

    /// Trusted device with elevated permissions
    Trusted,

    /// Admin device with full control
    Admin,
}

impl TrustLevel {
    /// Parse from string representation
    #[must_use]
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "trusted" => Self::Trusted,
            "admin" => Self::Admin,
            _ => Self::Paired,
        }
    }
}

impl std::fmt::Display for TrustLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Paired => write!(f, "paired"),
            Self::Trusted => write!(f, "trusted"),
            Self::Admin => write!(f, "admin"),
        }
    }
}

/// A paired device record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedDevice {
    /// Unique device identifier (from device identity)
    pub id: String,

    /// Ed25519 public key (base64 encoded)
    pub public_key: String,

    /// Human-readable device name
    pub name: String,

    /// Platform identifier (e.g., "linux-x86_64")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,

    /// Trust level for this device
    pub trust_level: TrustLevel,

    /// When the device was paired
    pub paired_at: DateTime<Utc>,

    /// When the device was last seen
    pub last_seen: DateTime<Utc>,
}

/// Manages paired device storage and operations
#[derive(Clone)]
pub struct DeviceManager {
    pool: DbPool,
}

const DEVICE_COLUMNS: &str = "id, public_key, name, platform, trust_level, paired_at, last_seen";

impl PairedDevice {
    fn from_row(row: &AnyRow) -> Result<Self> {
        Ok(Self {
            id: row.try_get(0)?,
            public_key: row.try_get(1)?,
            name: row.try_get(2)?,
            platform: row.try_get::<Option<String>, _>(3)?,
            trust_level: TrustLevel::from_str(&row.try_get::<String, _>(4)?),
            paired_at: parse_datetime(&row.try_get::<String, _>(5)?),
            last_seen: parse_datetime(&row.try_get::<String, _>(6)?),
        })
    }
}

impl DeviceManager {
    /// Create a new device manager
    #[must_use]
    pub const fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Register a new paired device
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails or device already exists
    pub async fn register(
        &self,
        device_id: &str,
        public_key: &str,
        name: &str,
        platform: Option<&str>,
        trust_level: TrustLevel,
    ) -> Result<PairedDevice> {
        let now = Utc::now();

        sqlx::query(
            "INSERT INTO devices (id, public_key, name, platform, trust_level, paired_at, last_seen)
             VALUES ($1, $2, $3, $4, $5, $6, $6)",
        )
        .bind(device_id.to_string())
        .bind(public_key.to_string())
        .bind(name.to_string())
        .bind(platform.unwrap_or("").to_string())
        .bind(trust_level.to_string())
        .bind(now.to_rfc3339())
        .execute(self.pool.pool())
        .await
        .map_err(|e| {
            if e.as_database_error()
                .is_some_and(sqlx::error::DatabaseError::is_unique_violation) {
                Error::Auth("device already paired".to_string())
            } else {
                Error::Database(e.to_string())
            }
        })?;

        tracing::info!(device_id, name, "device paired");

        Ok(PairedDevice {
            id: device_id.to_string(),
            public_key: public_key.to_string(),
            name: name.to_string(),
            platform: platform.map(ToString::to_string),
            trust_level,
            paired_at: now,
            last_seen: now,
        })
    }

    /// Get a device by ID
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn get(&self, device_id: &str) -> Result<Option<PairedDevice>> {
        self.find_by("id", device_id).await
    }

    /// Get a device by public key
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn get_by_public_key(&self, public_key: &str) -> Result<Option<PairedDevice>> {
        self.find_by("public_key", public_key).await
    }

    async fn find_by(&self, column: &'static str, value: &str) -> Result<Option<PairedDevice>> {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {DEVICE_COLUMNS} FROM devices WHERE {column} = $1"
        )))
        .bind(value.to_string())
        .fetch_optional(self.pool.pool())
        .await?
        .as_ref()
        .map(PairedDevice::from_row)
        .transpose()
    }

    /// Update last seen timestamp for a device
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn update_last_seen(&self, device_id: &str) -> Result<()> {
        sqlx::query("UPDATE devices SET last_seen = $1 WHERE id = $2")
            .bind(Utc::now().to_rfc3339())
            .bind(device_id.to_string())
            .execute(self.pool.pool())
            .await?;
        Ok(())
    }

    /// Update device trust level
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn update_trust_level(&self, device_id: &str, trust_level: TrustLevel) -> Result<()> {
        sqlx::query("UPDATE devices SET trust_level = $1 WHERE id = $2")
            .bind(trust_level.to_string())
            .bind(device_id.to_string())
            .execute(self.pool.pool())
            .await?;

        tracing::info!(device_id, %trust_level, "updated device trust level");
        Ok(())
    }

    /// Update device name
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn update_name(&self, device_id: &str, name: &str) -> Result<()> {
        sqlx::query("UPDATE devices SET name = $1 WHERE id = $2")
            .bind(name.to_string())
            .bind(device_id.to_string())
            .execute(self.pool.pool())
            .await?;
        Ok(())
    }

    /// Remove a device
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn remove(&self, device_id: &str) -> Result<bool> {
        let rows = sqlx::query("DELETE FROM devices WHERE id = $1")
            .bind(device_id.to_string())
            .execute(self.pool.pool())
            .await?
            .rows_affected();

        if rows > 0 {
            tracing::info!(device_id, "device removed");
        }

        Ok(rows > 0)
    }

    /// List all paired devices
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn list(&self) -> Result<Vec<PairedDevice>> {
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {DEVICE_COLUMNS} FROM devices ORDER BY last_seen DESC"
        )))
        .fetch_all(self.pool.pool())
        .await?;

        Ok(rows
            .iter()
            .filter_map(|row| PairedDevice::from_row(row).ok())
            .collect())
    }

    /// Count paired devices
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn count(&self) -> Result<usize> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM devices")
            .fetch_one(self.pool.pool())
            .await?;

        Ok(usize::try_from(count).unwrap_or(0))
    }

    /// Check if a device is paired (exists in database)
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn is_paired(&self, device_id: &str) -> Result<bool> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM devices WHERE id = $1")
            .bind(device_id.to_string())
            .fetch_one(self.pool.pool())
            .await?;

        Ok(count > 0)
    }
}

fn parse_datetime(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_memory;

    async fn setup() -> DeviceManager {
        let pool = init_memory().await.unwrap();
        DeviceManager::new(pool)
    }

    #[tokio::test]
    async fn test_register_device() {
        let manager = setup().await;

        let device = manager
            .register(
                "device123",
                "public_key_base64",
                "My Laptop",
                Some("linux-x86_64"),
                TrustLevel::Paired,
            )
            .await
            .unwrap();

        assert_eq!(device.id, "device123");
        assert_eq!(device.name, "My Laptop");
        assert_eq!(device.trust_level, TrustLevel::Paired);
    }

    #[tokio::test]
    async fn test_get_device() {
        let manager = setup().await;

        manager
            .register("device456", "pk456", "Phone", None, TrustLevel::Trusted)
            .await
            .unwrap();

        let device = manager.get("device456").await.unwrap().unwrap();
        assert_eq!(device.name, "Phone");
        assert_eq!(device.trust_level, TrustLevel::Trusted);

        // Non-existent device
        assert!(manager.get("nonexistent").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_duplicate_device_fails() {
        let manager = setup().await;

        manager
            .register("device789", "pk789", "Device 1", None, TrustLevel::Paired)
            .await
            .unwrap();

        // Same ID should fail
        let result = manager.register(
            "device789",
            "pk_different",
            "Device 2",
            None,
            TrustLevel::Paired,
        );
        assert!(result.await.is_err());

        // Same public key should fail
        let result = manager.register(
            "different_id",
            "pk789",
            "Device 3",
            None,
            TrustLevel::Paired,
        );
        assert!(result.await.is_err());
    }

    #[tokio::test]
    async fn test_update_trust_level() {
        let manager = setup().await;

        manager
            .register("deviceA", "pkA", "Test", None, TrustLevel::Paired)
            .await
            .unwrap();

        manager
            .update_trust_level("deviceA", TrustLevel::Admin)
            .await
            .unwrap();

        let device = manager.get("deviceA").await.unwrap().unwrap();
        assert_eq!(device.trust_level, TrustLevel::Admin);
    }

    #[tokio::test]
    async fn test_remove_device() {
        let manager = setup().await;

        manager
            .register("deviceB", "pkB", "Test", None, TrustLevel::Paired)
            .await
            .unwrap();

        assert!(manager.is_paired("deviceB").await.unwrap());
        assert!(manager.remove("deviceB").await.unwrap());
        assert!(!manager.is_paired("deviceB").await.unwrap());

        // Removing non-existent device returns false
        assert!(!manager.remove("nonexistent").await.unwrap());
    }

    #[tokio::test]
    async fn test_list_devices() {
        let manager = setup().await;

        manager
            .register("d1", "pk1", "Device 1", None, TrustLevel::Paired)
            .await
            .unwrap();
        manager
            .register("d2", "pk2", "Device 2", None, TrustLevel::Trusted)
            .await
            .unwrap();
        manager
            .register("d3", "pk3", "Device 3", None, TrustLevel::Admin)
            .await
            .unwrap();

        let devices = manager.list().await.unwrap();
        assert_eq!(devices.len(), 3);
    }

    #[tokio::test]
    async fn test_count_devices() {
        let manager = setup().await;

        assert_eq!(manager.count().await.unwrap(), 0);

        manager
            .register("d1", "pk1", "Device", None, TrustLevel::Paired)
            .await
            .unwrap();

        assert_eq!(manager.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_trust_level_parsing() {
        assert_eq!(TrustLevel::from_str("paired"), TrustLevel::Paired);
        assert_eq!(TrustLevel::from_str("trusted"), TrustLevel::Trusted);
        assert_eq!(TrustLevel::from_str("admin"), TrustLevel::Admin);
        assert_eq!(TrustLevel::from_str("unknown"), TrustLevel::Paired);
    }
}
