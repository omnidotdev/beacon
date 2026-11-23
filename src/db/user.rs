//! User repository for CRUD operations

use chrono::{DateTime, Utc};
use uuid::Uuid;

use sqlx::Row;
use sqlx::any::AnyRow;

use super::DbPool;
use crate::Result;

/// A user
#[derive(Debug, Clone)]
pub struct User {
    pub id: String,
    pub life_json_path: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// User context entry (learned preference)
#[derive(Debug, Clone)]
pub struct UserContext {
    pub id: String,
    pub user_id: String,
    pub key: String,
    pub value: String,
    pub source: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// User repository
#[derive(Clone)]
pub struct UserRepo {
    pool: DbPool,
}

const USER_COLUMNS: &str = "id, life_json_path, created_at, updated_at";

impl User {
    fn from_row(row: &AnyRow) -> Result<Self> {
        Ok(Self {
            id: row.try_get(0)?,
            life_json_path: row.try_get(1)?,
            created_at: parse_datetime(&row.try_get::<String, _>(2)?),
            updated_at: parse_datetime(&row.try_get::<String, _>(3)?),
        })
    }
}

impl UserRepo {
    /// Create a new user repository
    #[must_use]
    #[allow(clippy::missing_const_for_fn)]
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Find or create a user
    ///
    /// Inserts only when absent, so concurrent first requests for the same
    /// user cannot race into a duplicate key error
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn find_or_create(&self, id: &str) -> Result<User> {
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            "INSERT INTO users (id, created_at, updated_at) VALUES ($1, $2, $2)
             ON CONFLICT (id) DO NOTHING",
        )
        .bind(id.to_string())
        .bind(now)
        .execute(self.pool.pool())
        .await?;

        self.find(id)
            .await?
            .ok_or_else(|| crate::Error::Database(format!("user {id} missing after insert")))
    }

    /// Find a user by ID (returns None if not found)
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn find(&self, id: &str) -> Result<Option<User>> {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {USER_COLUMNS} FROM users WHERE id = $1"
        )))
        .bind(id.to_string())
        .fetch_optional(self.pool.pool())
        .await?
        .as_ref()
        .map(User::from_row)
        .transpose()
    }

    /// List all users
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn list_all(&self) -> Result<Vec<User>> {
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {USER_COLUMNS} FROM users ORDER BY created_at DESC"
        )))
        .fetch_all(self.pool.pool())
        .await?;

        Ok(rows
            .iter()
            .filter_map(|row| User::from_row(row).ok())
            .collect())
    }

    /// Delete a user and everything stored for them
    ///
    /// Removes their messages, sessions, memories (and memory vectors), and
    /// learned context first, in one transaction, so no row is left pointing at
    /// a user that no longer exists
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn delete(&self, id: &str) -> Result<()> {
        let mut tx = self.pool.pool().begin().await?;

        for statement in [
            "DELETE FROM messages WHERE session_id IN (SELECT id FROM sessions WHERE user_id = $1)",
            "DELETE FROM sessions WHERE user_id = $1",
            "DELETE FROM memories_vec WHERE memory_id IN (SELECT id FROM memories WHERE user_id = $1)",
            "DELETE FROM memories WHERE user_id = $1",
            "DELETE FROM user_context WHERE user_id = $1",
            "DELETE FROM users WHERE id = $1",
        ] {
            sqlx::query(statement)
                .bind(id.to_string())
                .execute(&mut *tx)
                .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    /// Set user's life.json path
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn set_life_json_path(&self, user_id: &str, path: Option<&str>) -> Result<()> {
        sqlx::query("UPDATE users SET life_json_path = $1, updated_at = $2 WHERE id = $3")
            .bind(path.map(str::to_string))
            .bind(Utc::now().to_rfc3339())
            .bind(user_id.to_string())
            .execute(self.pool.pool())
            .await?;

        Ok(())
    }

    /// Get user context (learned preferences)
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn get_context(&self, user_id: &str) -> Result<Vec<UserContext>> {
        let rows = sqlx::query(
            "SELECT id, user_id, key, value, source, created_at, updated_at
             FROM user_context WHERE user_id = $1 ORDER BY key",
        )
        .bind(user_id.to_string())
        .fetch_all(self.pool.pool())
        .await?;

        Ok(rows
            .iter()
            .filter_map(|row| {
                Some(UserContext {
                    id: row.try_get(0).ok()?,
                    user_id: row.try_get(1).ok()?,
                    key: row.try_get(2).ok()?,
                    value: row.try_get(3).ok()?,
                    source: row.try_get(4).ok()?,
                    created_at: parse_datetime(&row.try_get::<String, _>(5).ok()?),
                    updated_at: parse_datetime(&row.try_get::<String, _>(6).ok()?),
                })
            })
            .collect())
    }

    /// Set a user context value
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn set_context(
        &self,
        user_id: &str,
        key: &str,
        value: &str,
        source: &str,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO user_context (id, user_id, key, value, source, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $6)
             ON CONFLICT (user_id, key) DO UPDATE SET
                value = excluded.value,
                source = excluded.source,
                updated_at = excluded.updated_at",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(user_id.to_string())
        .bind(key.to_string())
        .bind(value.to_string())
        .bind(source.to_string())
        .bind(Utc::now().to_rfc3339())
        .execute(self.pool.pool())
        .await?;

        Ok(())
    }

    /// Get a specific context value
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn get_context_value(&self, user_id: &str, key: &str) -> Result<Option<String>> {
        Ok(
            sqlx::query_scalar("SELECT value FROM user_context WHERE user_id = $1 AND key = $2")
                .bind(user_id.to_string())
                .bind(key.to_string())
                .fetch_optional(self.pool.pool())
                .await?,
        )
    }
}

fn parse_datetime(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_memory;

    async fn setup() -> UserRepo {
        let pool = init_memory().await.unwrap();
        UserRepo::new(pool)
    }

    #[tokio::test]
    async fn test_find_or_create_user() {
        let repo = setup().await;

        let user = repo.find_or_create("user-123").await.unwrap();
        assert_eq!(user.id, "user-123");
        assert!(user.life_json_path.is_none());

        // Should return same user
        let user2 = repo.find_or_create("user-123").await.unwrap();
        assert_eq!(user.id, user2.id);
    }

    #[tokio::test]
    async fn test_set_life_json_path() {
        let repo = setup().await;

        let user = repo.find_or_create("user-456").await.unwrap();
        repo.set_life_json_path(&user.id, Some("/home/user/life.json"))
            .await
            .unwrap();

        let user = repo.find_or_create("user-456").await.unwrap();
        assert_eq!(user.life_json_path.as_deref(), Some("/home/user/life.json"));
    }

    #[tokio::test]
    async fn test_user_context() {
        let repo = setup().await;

        let user = repo.find_or_create("user-789").await.unwrap();

        // Set context
        repo.set_context(&user.id, "timezone", "America/Los_Angeles", "learned")
            .await
            .unwrap();
        repo.set_context(&user.id, "name", "Brian", "life.json")
            .await
            .unwrap();

        // Get all context
        let contexts = repo.get_context(&user.id).await.unwrap();
        assert_eq!(contexts.len(), 2);

        // Get specific value
        let tz = repo.get_context_value(&user.id, "timezone").await.unwrap();
        assert_eq!(tz, Some("America/Los_Angeles".to_string()));

        // Update existing context
        repo.set_context(&user.id, "timezone", "America/New_York", "learned")
            .await
            .unwrap();
        let tz = repo.get_context_value(&user.id, "timezone").await.unwrap();
        assert_eq!(tz, Some("America/New_York".to_string()));
    }
}
