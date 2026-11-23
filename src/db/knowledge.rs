//! Knowledge pack repository for installed knowledge persistence

use sqlx::Row;
use sqlx::any::AnyRow;
use uuid::Uuid;

use super::{Backend, DbPool};
use crate::Result;
use crate::persona::KnowledgePack;

/// Column list for all knowledge pack SELECT queries
const PACK_COLUMNS: &str = "id, name, version, source_namespace, description, tags, chunk_count, has_embeddings, content, installed_at, updated_at";

/// Map a database row to a `KnowledgePackRow`
fn row_to_pack_row(row: &AnyRow) -> Result<KnowledgePackRow> {
    Ok(KnowledgePackRow {
        id: row.try_get(0)?,
        name: row.try_get(1)?,
        version: row.try_get(2)?,
        source_namespace: row.try_get(3)?,
        description: row.try_get(4)?,
        tags: row.try_get(5)?,
        chunk_count: row.try_get(6)?,
        has_embeddings: row.try_get::<i64, _>(7)? != 0,
        content: row.try_get(8)?,
        installed_at: row.try_get(9)?,
        updated_at: row.try_get(10)?,
    })
}

/// A knowledge pack row stored in the database
#[derive(Debug, Clone)]
pub struct KnowledgePackRow {
    pub id: String,
    pub name: String,
    pub version: String,
    pub source_namespace: String,
    pub description: Option<String>,
    /// JSON array of tags
    pub tags: String,
    pub chunk_count: i64,
    pub has_embeddings: bool,
    /// Full pack JSON
    pub content: String,
    pub installed_at: String,
    pub updated_at: String,
}

/// Knowledge pack repository for database operations
#[derive(Clone)]
pub struct KnowledgePackRepo {
    pool: DbPool,
}

impl KnowledgePackRepo {
    /// Create a new knowledge pack repository
    #[must_use]
    pub const fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Install a knowledge pack
    ///
    /// Serializes the pack to JSON and stores it alongside metadata.
    /// If the pack includes pre-computed embeddings, they are inserted
    /// into the `knowledge_vec` table in the same transaction.
    ///
    /// # Errors
    ///
    /// Returns error if database operation or serialization fails
    pub async fn install(&self, pack: &KnowledgePack, namespace: &str) -> Result<String> {
        let id = format!("kp_{}", Uuid::new_v4());
        let tags_json = serde_json::to_string(&pack.tags)?;
        let content_json = serde_json::to_string(pack)?;

        #[allow(clippy::cast_possible_wrap)]
        let chunk_count = pack.chunks.len() as i64;
        let has_embeddings = pack.embeddings.is_some();
        let now = super::now();

        let mut tx = self.pool.pool().begin().await?;

        sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT INTO installed_knowledge_packs ({PACK_COLUMNS})
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $10)"
        )))
        .bind(id.clone())
        .bind(pack.name.clone())
        .bind(pack.version.clone())
        .bind(namespace.to_string())
        .bind(pack.description.clone())
        .bind(tags_json)
        .bind(chunk_count)
        .bind(i64::from(has_embeddings))
        .bind(content_json)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        // Insert embeddings into knowledge_vec if present
        if let Some(ref embeddings) = pack.embeddings {
            let insert = match self.pool.backend() {
                Backend::Sqlite => {
                    "INSERT INTO knowledge_vec (chunk_id, embedding) VALUES ($1, $2)"
                }
                Backend::Postgres => {
                    "INSERT INTO knowledge_vec (chunk_id, embedding) VALUES ($1, $2::vector)"
                }
            };
            for (chunk_idx, embedding) in &embeddings.vectors {
                let query = sqlx::query(insert).bind(format!("{id}:{chunk_idx}"));
                super::encode_embedding(self.pool.backend(), embedding)
                    .bind(query)
                    .execute(&mut *tx)
                    .await?;
            }
        }

        tx.commit().await?;

        tracing::info!(
            pack_id = %id,
            name = %pack.name,
            namespace = %namespace,
            chunk_count,
            has_embeddings,
            "knowledge pack installed"
        );

        Ok(id)
    }

    /// Uninstall a knowledge pack by name and namespace
    ///
    /// Removes the pack row and cleans up any associated vector entries.
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn uninstall(&self, name: &str, namespace: &str) -> Result<bool> {
        let mut tx = self.pool.pool().begin().await?;

        // Look up the pack ID before deletion (needed for vector cleanup)
        let pack_id: Option<String> = sqlx::query_scalar(
            "SELECT id FROM installed_knowledge_packs WHERE name = $1 AND source_namespace = $2",
        )
        .bind(name.to_string())
        .bind(namespace.to_string())
        .fetch_optional(&mut *tx)
        .await?;

        let Some(pack_id) = pack_id else {
            return Ok(false);
        };

        // Clean up vector entries (chunk_id starts with pack_id)
        sqlx::query("DELETE FROM knowledge_vec WHERE chunk_id LIKE $1")
            .bind(format!("{pack_id}:%"))
            .execute(&mut *tx)
            .await?;

        // Delete the pack row
        let deleted = sqlx::query("DELETE FROM installed_knowledge_packs WHERE id = $1")
            .bind(pack_id.clone())
            .execute(&mut *tx)
            .await?
            .rows_affected();

        tx.commit().await?;

        if deleted > 0 {
            tracing::info!(
                pack_id = %pack_id,
                name = %name,
                namespace = %namespace,
                "knowledge pack uninstalled"
            );
        }

        Ok(deleted > 0)
    }

    /// Get a knowledge pack by ID
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn get(&self, id: &str) -> Result<Option<KnowledgePackRow>> {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {PACK_COLUMNS} FROM installed_knowledge_packs WHERE id = $1"
        )))
        .bind(id.to_string())
        .fetch_optional(self.pool.pool())
        .await?
        .as_ref()
        .map(row_to_pack_row)
        .transpose()
    }

    /// Get a knowledge pack by name and namespace
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn get_by_name(
        &self,
        name: &str,
        namespace: &str,
    ) -> Result<Option<KnowledgePackRow>> {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {PACK_COLUMNS} FROM installed_knowledge_packs WHERE name = $1 AND source_namespace = $2"
        )))
        .bind(name.to_string())
        .bind(namespace.to_string())
        .fetch_optional(self.pool.pool())
        .await?
        .as_ref()
        .map(row_to_pack_row)
        .transpose()
    }

    /// List all installed knowledge packs
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn list(&self) -> Result<Vec<KnowledgePackRow>> {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {PACK_COLUMNS} FROM installed_knowledge_packs ORDER BY name"
        )))
        .fetch_all(self.pool.pool())
        .await?
        .iter()
        .map(row_to_pack_row)
        .collect()
    }

    /// Search knowledge chunk embeddings by vector similarity
    ///
    /// Returns `(chunk_id, distance)` pairs ordered by L2 distance (closest first).
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn search_chunks(
        &self,
        query_embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<(String, f32)>> {
        let sql = match self.pool.backend() {
            Backend::Sqlite => {
                r"SELECT chunk_id, distance
                  FROM knowledge_vec
                  WHERE embedding MATCH $1
                  ORDER BY distance
                  LIMIT $2"
            }
            Backend::Postgres => {
                r"SELECT chunk_id, CAST(embedding <-> $1::vector AS DOUBLE PRECISION) AS distance
                  FROM knowledge_vec
                  ORDER BY embedding <-> $1::vector
                  LIMIT $2"
            }
        };

        #[allow(clippy::cast_possible_wrap)]
        let rows = super::encode_embedding(self.pool.backend(), query_embedding)
            .bind(sqlx::query(sql))
            .bind(limit as i64)
            .fetch_all(self.pool.pool())
            .await?;

        #[allow(clippy::cast_possible_truncation)]
        rows.iter()
            .map(|row| Ok((row.try_get(0)?, row.try_get::<f64, _>(1)? as f32)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_memory;
    use crate::persona::{KnowledgeChunk, KnowledgePriority};

    fn test_pack() -> KnowledgePack {
        KnowledgePack {
            schema: None,
            version: "1.0.0".to_string(),
            name: "test-knowledge".to_string(),
            description: Some("A test knowledge pack".to_string()),
            tags: vec!["test".to_string(), "example".to_string()],
            chunks: vec![
                KnowledgeChunk {
                    topic: Some("Rust basics".to_string()),
                    tags: vec!["rust".to_string()],
                    content: "Rust is a systems programming language".to_string(),
                    rules: vec![],
                    priority: KnowledgePriority::Relevant,
                    embedding: None,
                },
                KnowledgeChunk {
                    topic: Some("Testing".to_string()),
                    tags: vec!["testing".to_string()],
                    content: "Write tests for your code".to_string(),
                    rules: vec![],
                    priority: KnowledgePriority::Always,
                    embedding: None,
                },
            ],
            embeddings: None,
        }
    }

    #[tokio::test]
    async fn test_install_and_get() {
        let pool = init_memory().await.unwrap();
        let repo = KnowledgePackRepo::new(pool);

        let pack = test_pack();
        let id = repo.install(&pack, "omni").await.unwrap();
        assert!(id.starts_with("kp_"));

        let fetched = repo.get(&id).await.unwrap().unwrap();
        assert_eq!(fetched.name, "test-knowledge");
        assert_eq!(fetched.version, "1.0.0");
        assert_eq!(fetched.source_namespace, "omni");
        assert_eq!(fetched.chunk_count, 2);
        assert!(!fetched.has_embeddings);
    }

    #[tokio::test]
    async fn test_get_by_name() {
        let pool = init_memory().await.unwrap();
        let repo = KnowledgePackRepo::new(pool);

        let pack = test_pack();
        repo.install(&pack, "omni").await.unwrap();

        let fetched = repo
            .get_by_name("test-knowledge", "omni")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fetched.name, "test-knowledge");

        // Different namespace should not match
        let not_found = repo.get_by_name("test-knowledge", "other").await.unwrap();
        assert!(not_found.is_none());
    }

    #[tokio::test]
    async fn test_list() {
        let pool = init_memory().await.unwrap();
        let repo = KnowledgePackRepo::new(pool);

        let pack = test_pack();
        repo.install(&pack, "omni").await.unwrap();

        let packs = repo.list().await.unwrap();
        assert_eq!(packs.len(), 1);
        assert_eq!(packs[0].name, "test-knowledge");
    }

    #[tokio::test]
    async fn test_uninstall() {
        let pool = init_memory().await.unwrap();
        let repo = KnowledgePackRepo::new(pool);

        let pack = test_pack();
        let id = repo.install(&pack, "omni").await.unwrap();

        // Verify it exists
        assert!(repo.get(&id).await.unwrap().is_some());

        // Uninstall
        let removed = repo.uninstall("test-knowledge", "omni").await.unwrap();
        assert!(removed);

        // Verify it's gone
        assert!(repo.get(&id).await.unwrap().is_none());

        // Uninstalling again should return false
        let removed_again = repo.uninstall("test-knowledge", "omni").await.unwrap();
        assert!(!removed_again);
    }

    #[tokio::test]
    async fn test_unique_constraint() {
        let pool = init_memory().await.unwrap();
        let repo = KnowledgePackRepo::new(pool);

        let pack = test_pack();
        repo.install(&pack, "omni").await.unwrap();

        // Installing the same name+namespace again should fail
        let result = repo.install(&pack, "omni");
        assert!(result.await.is_err());
    }

    #[tokio::test]
    async fn test_content_roundtrip() {
        let pool = init_memory().await.unwrap();
        let repo = KnowledgePackRepo::new(pool);

        let pack = test_pack();
        let id = repo.install(&pack, "omni").await.unwrap();

        let fetched = repo.get(&id).await.unwrap().unwrap();
        let deserialized: KnowledgePack = serde_json::from_str(&fetched.content).unwrap();
        assert_eq!(deserialized.name, "test-knowledge");
        assert_eq!(deserialized.chunks.len(), 2);
        assert_eq!(deserialized.chunks[0].topic.as_deref(), Some("Rust basics"));
    }
}
