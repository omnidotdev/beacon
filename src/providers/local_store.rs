//! Local `SQLite` storage for gateway-level provider keys (self-hosted deployments)

use crate::Result;
use crate::db::DbPool;

/// A stored provider key from the local database
#[derive(Debug, Clone)]
pub struct StoredKey {
    pub api_key: String,
    pub model_preference: Option<String>,
}

/// Gateway-local key store — used when Synapse is not configured
#[derive(Clone)]
pub struct LocalKeyStore {
    db: DbPool,
}

impl LocalKeyStore {
    /// Create a new local key store backed by the given pool
    #[must_use]
    pub const fn new(db: DbPool) -> Self {
        Self { db }
    }

    /// Retrieve the stored key for a provider, or `None` if not configured
    ///
    /// # Errors
    ///
    /// Returns an error if the database query fails.
    pub async fn get(&self, provider: &str) -> Result<Option<StoredKey>> {
        use sqlx::Row;

        let row = sqlx::query(
            "SELECT api_key, model_preference FROM local_provider_keys WHERE provider = $1",
        )
        .bind(provider.to_string())
        .fetch_optional(self.db.pool())
        .await?;

        row.map(|row| {
            Ok(StoredKey {
                api_key: row.try_get(0)?,
                model_preference: row.try_get(1)?,
            })
        })
        .transpose()
    }

    /// Insert or replace the key for a provider
    ///
    /// # Errors
    ///
    /// Returns an error if the database write fails.
    pub async fn set(
        &self,
        provider: &str,
        api_key: &str,
        model_preference: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO local_provider_keys (provider, api_key, model_preference, updated_at)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (provider) DO UPDATE SET
                api_key = excluded.api_key,
                model_preference = excluded.model_preference,
                updated_at = excluded.updated_at",
        )
        .bind(provider.to_string())
        .bind(api_key.to_string())
        .bind(model_preference.map(str::to_string))
        .bind(crate::db::now())
        .execute(self.db.pool())
        .await?;
        Ok(())
    }

    /// Remove the stored key for a provider
    ///
    /// # Errors
    ///
    /// Returns an error if the database write fails.
    pub async fn remove(&self, provider: &str) -> Result<()> {
        sqlx::query("DELETE FROM local_provider_keys WHERE provider = $1")
            .bind(provider.to_string())
            .execute(self.db.pool())
            .await?;
        Ok(())
    }

    /// Return all provider names that have a locally stored key
    ///
    /// # Errors
    ///
    /// Returns an error if the database query fails.
    pub async fn list_configured(&self) -> Result<Vec<String>> {
        Ok(
            sqlx::query_scalar("SELECT provider FROM local_provider_keys")
                .fetch_all(self.db.pool())
                .await?,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_db() -> crate::db::DbPool {
        crate::db::init_memory().await.unwrap()
    }

    #[tokio::test]
    async fn set_and_get_key() {
        let store = LocalKeyStore::new(test_db().await);
        store.set("anthropic", "sk-ant-test", None).await.unwrap();
        let key = store.get("anthropic").await.unwrap();
        assert_eq!(key.unwrap().api_key, "sk-ant-test");
    }

    #[tokio::test]
    async fn remove_key() {
        let store = LocalKeyStore::new(test_db().await);
        store.set("openai", "sk-openai-test", None).await.unwrap();
        store.remove("openai").await.unwrap();
        assert!(store.get("openai").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn list_configured_providers() {
        let store = LocalKeyStore::new(test_db().await);
        store.set("anthropic", "sk-ant-test", None).await.unwrap();
        store.set("openai", "sk-openai-test", None).await.unwrap();
        let configured = store.list_configured().await.unwrap();
        assert!(configured.contains(&"anthropic".to_string()));
        assert!(configured.contains(&"openai".to_string()));
        assert!(!configured.contains(&"openrouter".to_string()));
    }
}
