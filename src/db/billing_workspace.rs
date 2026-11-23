//! Billing workspace repository
//!
//! Remembers the personal workspace each Gatekeeper user's usage bills, so
//! work that runs without the user's token (scheduled check-ins, workflows)
//! keeps billing it after a restart instead of having nothing to bill

use super::DbPool;
use crate::Result;

/// Repository for each user's billing workspace
#[derive(Debug, Clone)]
pub struct BillingWorkspaceRepo {
    pool: DbPool,
}

impl BillingWorkspaceRepo {
    /// Create a new repository
    #[must_use]
    pub const fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// The workspace a user's usage bills, if one has been recorded
    ///
    /// # Errors
    ///
    /// Returns error if the database operation fails
    pub async fn get(&self, user_id: &str) -> Result<Option<String>> {
        Ok(
            sqlx::query_scalar("SELECT workspace_id FROM billing_workspaces WHERE user_id = $1")
                .bind(user_id.to_string())
                .fetch_optional(self.pool.pool())
                .await?,
        )
    }

    /// Record the workspace a user's usage bills
    ///
    /// # Errors
    ///
    /// Returns error if the database operation fails
    pub async fn set(&self, user_id: &str, workspace_id: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO billing_workspaces (user_id, workspace_id, updated_at)
             VALUES ($1, $2, $3)
             ON CONFLICT (user_id) DO UPDATE SET
                workspace_id = excluded.workspace_id,
                updated_at = excluded.updated_at",
        )
        .bind(user_id.to_string())
        .bind(workspace_id.to_string())
        .bind(super::now())
        .execute(self.pool.pool())
        .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[tokio::test]
    async fn records_and_replaces_a_users_workspace() {
        let repo = BillingWorkspaceRepo::new(db::init_memory().await.unwrap());

        assert_eq!(repo.get("u-1").await.unwrap(), None);

        repo.set("u-1", "ws-1").await.unwrap();
        assert_eq!(repo.get("u-1").await.unwrap().as_deref(), Some("ws-1"));

        repo.set("u-1", "ws-2").await.unwrap();
        assert_eq!(repo.get("u-1").await.unwrap().as_deref(), Some("ws-2"));
        assert_eq!(repo.get("u-2").await.unwrap(), None);
    }
}
