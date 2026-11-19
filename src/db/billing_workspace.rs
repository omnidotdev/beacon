//! Billing workspace repository
//!
//! Remembers the personal workspace each Gatekeeper user's usage bills, so
//! work that runs without the user's token (scheduled check-ins, workflows)
//! keeps billing it after a restart instead of having nothing to bill

use super::DbPool;
use crate::{Error, Result};

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
    pub fn get(&self, user_id: &str) -> Result<Option<String>> {
        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;

        match conn.query_row(
            "SELECT workspace_id FROM billing_workspaces WHERE user_id = ?1",
            [user_id],
            |row| row.get(0),
        ) {
            Ok(workspace_id) => Ok(Some(workspace_id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(Error::Database(e.to_string())),
        }
    }

    /// Record the workspace a user's usage bills
    ///
    /// # Errors
    ///
    /// Returns error if the database operation fails
    pub fn set(&self, user_id: &str, workspace_id: &str) -> Result<()> {
        let conn = self
            .pool
            .get()
            .map_err(|e| Error::Database(e.to_string()))?;

        conn.execute(
            "INSERT INTO billing_workspaces (user_id, workspace_id, updated_at)
             VALUES (?1, ?2, datetime('now'))
             ON CONFLICT(user_id) DO UPDATE SET
                workspace_id = excluded.workspace_id,
                updated_at = excluded.updated_at",
            [user_id, workspace_id],
        )
        .map_err(|e| Error::Database(e.to_string()))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[test]
    fn records_and_replaces_a_users_workspace() {
        let repo = BillingWorkspaceRepo::new(db::init_memory().unwrap());

        assert_eq!(repo.get("u-1").unwrap(), None);

        repo.set("u-1", "ws-1").unwrap();
        assert_eq!(repo.get("u-1").unwrap().as_deref(), Some("ws-1"));

        repo.set("u-1", "ws-2").unwrap();
        assert_eq!(repo.get("u-1").unwrap().as_deref(), Some("ws-2"));
        assert_eq!(repo.get("u-2").unwrap(), None);
    }
}
