//! Persona repository for installed personas persistence

use chrono::{DateTime, Utc};
use uuid::Uuid;

use sqlx::Row;
use sqlx::any::AnyRow;

use super::DbPool;
use crate::{Error, Persona, Result};

/// Installed persona from marketplace
#[derive(Debug, Clone)]
pub struct InstalledPersona {
    pub id: String,
    pub persona: Persona,
    pub source_namespace: String,
    pub installed_at: DateTime<Utc>,
}

/// Persona repository for CRUD operations on installed personas
#[derive(Clone)]
pub struct PersonaRepo {
    pool: DbPool,
}

impl PersonaRepo {
    /// Create a new persona repository
    #[must_use]
    pub const fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Install a persona from marketplace
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn install(&self, persona: &Persona, namespace: &str) -> Result<InstalledPersona> {
        let id = Uuid::new_v4().to_string();
        let content = serde_json::to_string(persona).map_err(|e| Error::Database(e.to_string()))?;

        let avatar = persona
            .branding
            .as_ref()
            .and_then(|b| b.assets.as_ref())
            .and_then(|a| a.avatar.clone());

        let accent_color = persona
            .branding
            .as_ref()
            .and_then(|b| b.colors.as_ref())
            .and_then(|c| c.primary.clone());

        sqlx::query(
            r"
            INSERT INTO installed_personas (
                id, persona_id, name, tagline, avatar, accent_color, content, source_namespace
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (persona_id) DO UPDATE SET
                name = excluded.name,
                tagline = excluded.tagline,
                avatar = excluded.avatar,
                accent_color = excluded.accent_color,
                content = excluded.content,
                updated_at = $9
            ",
        )
        .bind(id.clone())
        .bind(persona.identity.id.clone())
        .bind(persona.identity.name.clone())
        .bind(persona.identity.tagline.clone())
        .bind(avatar)
        .bind(accent_color)
        .bind(content)
        .bind(namespace.to_string())
        .bind(super::now())
        .execute(self.pool.pool())
        .await?;

        tracing::info!(persona_id = %persona.identity.id, name = %persona.identity.name, "persona installed");

        Ok(InstalledPersona {
            id,
            persona: persona.clone(),
            source_namespace: namespace.to_string(),
            installed_at: Utc::now(),
        })
    }

    /// Uninstall a persona by `persona_id`
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn uninstall(&self, persona_id: &str) -> Result<bool> {
        let rows = sqlx::query("DELETE FROM installed_personas WHERE persona_id = $1")
            .bind(persona_id.to_string())
            .execute(self.pool.pool())
            .await?
            .rows_affected();

        if rows > 0 {
            tracing::info!(persona_id = %persona_id, "persona uninstalled");
        }

        Ok(rows > 0)
    }

    /// Get an installed persona by `persona_id`
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn get(&self, persona_id: &str) -> Result<Option<InstalledPersona>> {
        sqlx::query(
            r"
            SELECT id, persona_id, content, source_namespace, installed_at
            FROM installed_personas
            WHERE persona_id = $1
            ",
        )
        .bind(persona_id.to_string())
        .fetch_optional(self.pool.pool())
        .await?
        .as_ref()
        .map(Self::row_to_installed_persona)
        .transpose()
    }

    /// List all installed personas
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn list(&self) -> Result<Vec<InstalledPersona>> {
        sqlx::query(
            r"
            SELECT id, persona_id, content, source_namespace, installed_at
            FROM installed_personas
            ORDER BY name
            ",
        )
        .fetch_all(self.pool.pool())
        .await?
        .iter()
        .map(Self::row_to_installed_persona)
        .collect()
    }

    /// Convert a database row to an `InstalledPersona`
    fn row_to_installed_persona(row: &AnyRow) -> Result<InstalledPersona> {
        let id: String = row.try_get(0)?;
        let content: String = row.try_get(2)?;
        let source_namespace: String = row.try_get(3)?;
        let installed_at: String = row.try_get(4)?;

        let persona: Persona =
            serde_json::from_str(&content).map_err(|e| Error::Database(e.to_string()))?;

        let installed_at = DateTime::parse_from_rfc3339(&format!("{installed_at}Z"))
            .map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc));

        Ok(InstalledPersona {
            id,
            persona,
            source_namespace,
            installed_at,
        })
    }
}
