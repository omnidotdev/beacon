//! Memory repository for long-term memory storage

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use sqlx::Row;
use sqlx::any::AnyRow;

use super::{Backend, DbPool};
use crate::Result;

/// Half-life for temporal decay (in days)
const DECAY_HALF_LIFE_DAYS: f64 = 7.0;

/// Weight of temporal decay in combined scoring (0.0 = no decay effect, 1.0 = full effect)
const DECAY_WEIGHT: f64 = 0.3;

/// Compute temporal decay factor for a memory based on its `accessed_at` timestamp.
///
/// Uses exponential decay with a configurable half-life.
/// Returns a value in `[0.0, 1.0]` where 1.0 means just accessed and ~0.0 means very old.
#[must_use]
pub fn temporal_decay_factor(accessed_at: &DateTime<Utc>, now: &DateTime<Utc>) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let elapsed_days = (*now - *accessed_at).num_seconds().max(0) as f64 / 86400.0;
    // Exponential decay: 2^(-t/half_life)
    (-elapsed_days / DECAY_HALF_LIFE_DAYS).exp2()
}

use agent_core::knowledge::cosine_similarity;

/// Candidate for MMR re-ranking
struct MmrCandidate {
    memory: Memory,
    score: f64,
}

/// Apply Maximal Marginal Relevance re-ranking to a list of scored candidates.
///
/// Iteratively selects candidates that balance relevance (score) with diversity
/// (dissimilarity to already-selected items). Lambda controls the trade-off:
/// higher lambda favors relevance, lower lambda favors diversity.
fn mmr_rerank(candidates: Vec<MmrCandidate>, limit: usize, lambda: f64) -> Vec<Memory> {
    if candidates.is_empty() || limit == 0 {
        return Vec::new();
    }

    let mut remaining: Vec<MmrCandidate> = candidates;
    let mut selected: Vec<(Memory, Option<Vec<f32>>)> = Vec::with_capacity(limit);

    // Normalize scores to [0, 1] for MMR calculation
    let max_score = remaining
        .iter()
        .map(|c| c.score)
        .fold(f64::NEG_INFINITY, f64::max);
    let min_score = remaining
        .iter()
        .map(|c| c.score)
        .fold(f64::INFINITY, f64::min);
    let score_range = max_score - min_score;

    while selected.len() < limit && !remaining.is_empty() {
        let mut best_idx = 0;
        let mut best_mmr = f64::NEG_INFINITY;

        for (i, candidate) in remaining.iter().enumerate() {
            let relevance = if score_range > f64::EPSILON {
                (candidate.score - min_score) / score_range
            } else {
                1.0
            };

            // Max similarity to any already-selected item
            let max_sim = if selected.is_empty() {
                0.0
            } else {
                candidate.memory.embedding.as_ref().map_or(0.0, |emb| {
                    selected
                        .iter()
                        .filter_map(|(_, sel_emb)| {
                            sel_emb
                                .as_ref()
                                .map(|se| f64::from(cosine_similarity(emb, se)))
                        })
                        .fold(0.0_f64, f64::max)
                })
            };

            let mmr_score = lambda.mul_add(relevance, -((1.0 - lambda) * max_sim));
            if mmr_score > best_mmr {
                best_mmr = mmr_score;
                best_idx = i;
            }
        }

        let chosen = remaining.swap_remove(best_idx);
        let emb = chosen.memory.embedding.clone();
        selected.push((chosen.memory, emb));
    }

    selected.into_iter().map(|(m, _)| m).collect()
}

/// Column list for all memory SELECT queries
const MEMORY_COLUMNS: &str = "id, user_id, category, content, tags, pinned, access_count, created_at, accessed_at, embedding, source_session_id, source_channel, content_hash, origin_device_id, updated_at, deleted_at, synced_at, cloud_id";

/// Memories from fetched rows, skipping any row that fails to decode
fn memories_from_rows(rows: &[AnyRow]) -> Vec<Memory> {
    rows.iter()
        .filter_map(|row| row_to_memory_row(row).ok())
        .map(MemoryRow::into_memory)
        .collect()
}

/// Insert or replace a memory's vector
///
/// sqlite-vec tables are virtual, and `SQLite` has no upsert for virtual
/// tables, so there the old vector is deleted first; pgvector upserts
async fn upsert_vector(
    conn: &mut sqlx::AnyConnection,
    backend: Backend,
    memory_id: &str,
    embedding: &[f32],
) -> Result<()> {
    let sql = match backend {
        Backend::Sqlite => {
            sqlx::query("DELETE FROM memories_vec WHERE memory_id = $1")
                .bind(memory_id.to_string())
                .execute(&mut *conn)
                .await?;
            "INSERT INTO memories_vec (memory_id, embedding) VALUES ($1, $2)"
        }
        Backend::Postgres => {
            "INSERT INTO memories_vec (memory_id, embedding) VALUES ($1, $2::vector)
             ON CONFLICT (memory_id) DO UPDATE SET embedding = excluded.embedding"
        }
    };

    super::encode_embedding(backend, embedding)
        .bind(sqlx::query(sql).bind(memory_id.to_string()))
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// Map a database row to a `MemoryRow`
fn row_to_memory_row(row: &AnyRow) -> Result<MemoryRow> {
    Ok(MemoryRow {
        id: row.try_get(0)?,
        user_id: row.try_get(1)?,
        category: row.try_get(2)?,
        content: row.try_get(3)?,
        tags: row.try_get(4)?,
        pinned: row.try_get(5)?,
        access_count: row.try_get(6)?,
        created_at: row.try_get(7)?,
        accessed_at: row.try_get(8)?,
        embedding: row.try_get(9)?,
        source_session_id: row.try_get(10)?,
        source_channel: row.try_get(11)?,
        content_hash: row.try_get(12)?,
        origin_device_id: row.try_get(13)?,
        updated_at: row.try_get(14)?,
        deleted_at: row.try_get(15)?,
        synced_at: row.try_get(16)?,
        cloud_id: row.try_get(17)?,
    })
}

/// Memory categories
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryCategory {
    /// User preferences (how they like things done)
    Preference,
    /// Facts about the user or their environment
    Fact,
    /// Corrections from user feedback
    Correction,
    /// General learned information
    General,
}

impl MemoryCategory {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Preference => "preference",
            Self::Fact => "fact",
            Self::Correction => "correction",
            Self::General => "general",
        }
    }

    #[must_use]
    pub fn from_str_value(s: &str) -> Option<Self> {
        match s {
            "preference" => Some(Self::Preference),
            "fact" => Some(Self::Fact),
            "correction" => Some(Self::Correction),
            "general" => Some(Self::General),
            _ => None,
        }
    }
}

impl std::fmt::Display for MemoryCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// A memory item stored in the database
#[derive(Debug, Clone)]
pub struct Memory {
    pub id: String,
    pub user_id: String,
    pub category: MemoryCategory,
    pub content: String,
    pub tags: Vec<String>,
    pub pinned: bool,
    pub access_count: u32,
    pub created_at: DateTime<Utc>,
    pub accessed_at: DateTime<Utc>,
    /// Optional embedding for vector search
    pub embedding: Option<Vec<f32>>,
    /// Source session ID (where this memory was learned)
    pub source_session_id: Option<String>,
    /// Source channel (where this memory was learned)
    pub source_channel: Option<String>,
    /// SHA-256 hash of content for dedup across devices
    pub content_hash: Option<String>,
    /// Device ID that originally created this memory
    pub origin_device_id: Option<String>,
    /// Last time this memory was modified
    pub updated_at: String,
    /// Soft-delete tombstone timestamp
    pub deleted_at: Option<String>,
    /// Last time this memory was synced to the cloud
    pub synced_at: Option<String>,
    /// API-side UUID for cross-device identity
    pub cloud_id: Option<String>,
}

impl Memory {
    /// Create a new memory item
    #[must_use]
    pub fn new(user_id: String, category: MemoryCategory, content: String) -> Self {
        let now = Utc::now();
        let content_hash = Self::compute_content_hash(&content);
        Self {
            id: format!("mem_{}", Uuid::new_v4()),
            user_id,
            category,
            content,
            tags: Vec::new(),
            pinned: false,
            access_count: 0,
            created_at: now,
            accessed_at: now,
            embedding: None,
            source_session_id: None,
            source_channel: None,
            content_hash: Some(content_hash),
            origin_device_id: None,
            updated_at: now.to_rfc3339(),
            deleted_at: None,
            synced_at: None,
            cloud_id: None,
        }
    }

    /// Compute SHA-256 hash of content for dedup
    #[must_use]
    pub fn compute_content_hash(content: &str) -> String {
        use sha2::{Digest, Sha256};

        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        hex::encode(hasher.finalize())
    }

    /// Add a tag to this memory
    #[must_use]
    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Mark this memory as pinned
    #[must_use]
    pub const fn pinned(mut self) -> Self {
        self.pinned = true;
        self
    }

    /// Set the embedding for this memory
    #[must_use]
    pub fn with_embedding(mut self, embedding: Vec<f32>) -> Self {
        self.embedding = Some(embedding);
        self
    }

    /// Set the source session and channel
    #[must_use]
    pub fn with_source(mut self, session_id: String, channel: String) -> Self {
        self.source_session_id = Some(session_id);
        self.source_channel = Some(channel);
        self
    }
}

/// Memory repository for database operations
#[derive(Debug, Clone)]
pub struct MemoryRepo {
    pool: DbPool,
}

impl MemoryRepo {
    /// Create a new memory repository
    #[must_use]
    pub const fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Add a new memory
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn add(&self, memory: &Memory) -> Result<()> {
        let mut tx = self.pool.pool().begin().await?;
        Self::insert(&mut tx, self.pool.backend(), memory).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Insert a memory row (and its vector) inside a transaction
    async fn insert(
        tx: &mut sqlx::Transaction<'_, sqlx::Any>,
        backend: Backend,
        memory: &Memory,
    ) -> Result<()> {
        let tags_json = serde_json::to_string(&memory.tags).unwrap_or_else(|_| "[]".to_string());

        // Convert embedding to bytes if present
        let embedding_bytes = memory
            .embedding
            .as_ref()
            .map(|e| super::embedder::Embedder::to_bytes(e));

        sqlx::query(
            r"INSERT INTO memories (id, user_id, category, content, tags, pinned, access_count, created_at, accessed_at, embedding, source_session_id, source_channel, content_hash, origin_device_id, updated_at, deleted_at, synced_at, cloud_id)
              VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18)",
        )
        .bind(memory.id.clone())
        .bind(memory.user_id.clone())
        .bind(memory.category.as_str())
        .bind(memory.content.clone())
        .bind(tags_json)
        .bind(i64::from(memory.pinned))
        .bind(i64::from(memory.access_count))
        .bind(memory.created_at.to_rfc3339())
        .bind(memory.accessed_at.to_rfc3339())
        .bind(embedding_bytes)
        .bind(memory.source_session_id.clone())
        .bind(memory.source_channel.clone())
        .bind(memory.content_hash.clone())
        .bind(memory.origin_device_id.clone())
        .bind(memory.updated_at.clone())
        .bind(memory.deleted_at.clone())
        .bind(memory.synced_at.clone())
        .bind(memory.cloud_id.clone())
        .execute(&mut **tx)
        .await?;

        // Also insert into vector table if embedding is present
        if let Some(ref embedding) = memory.embedding {
            upsert_vector(tx, backend, &memory.id, embedding).await?;
        }

        Ok(())
    }

    /// Get a memory by ID (and update access stats)
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn get(&self, id: &str) -> Result<Option<Memory>> {
        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {MEMORY_COLUMNS} FROM memories WHERE id = $1 AND deleted_at IS NULL"
        )))
        .bind(id.to_string())
        .fetch_optional(self.pool.pool())
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };
        let memory = row_to_memory_row(&row)?;

        // Update access stats (RFC 3339, the format memories are created with
        // and parsed back in)
        sqlx::query(
            "UPDATE memories SET access_count = access_count + 1, accessed_at = $1 WHERE id = $2",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(id.to_string())
        .execute(self.pool.pool())
        .await?;

        Ok(Some(memory.into_memory()))
    }

    /// Get a memory by ID without updating access stats (for sync operations)
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn get_without_access_update(&self, id: &str) -> Result<Option<Memory>> {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {MEMORY_COLUMNS} FROM memories WHERE id = $1"
        )))
        .bind(id.to_string())
        .fetch_optional(self.pool.pool())
        .await?
        .as_ref()
        .map(|row| row_to_memory_row(row).map(MemoryRow::into_memory))
        .transpose()
    }

    /// List memories for a user, optionally filtered by category
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn list(
        &self,
        user_id: &str,
        category: Option<MemoryCategory>,
    ) -> Result<Vec<Memory>> {
        let rows = if let Some(cat) = category {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "SELECT {MEMORY_COLUMNS} FROM memories
                 WHERE user_id = $1 AND category = $2 AND deleted_at IS NULL
                 ORDER BY pinned DESC, accessed_at DESC"
            )))
            .bind(user_id.to_string())
            .bind(cat.as_str())
            .fetch_all(self.pool.pool())
            .await?
        } else {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "SELECT {MEMORY_COLUMNS} FROM memories
                 WHERE user_id = $1 AND deleted_at IS NULL
                 ORDER BY pinned DESC, accessed_at DESC"
            )))
            .bind(user_id.to_string())
            .fetch_all(self.pool.pool())
            .await?
        };

        Ok(memories_from_rows(&rows))
    }

    /// Search memories by content (simple substring match)
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn search(&self, user_id: &str, query: &str) -> Result<Vec<Memory>> {
        // Lowercase both sides: SQLite's LIKE ignores ASCII case, Postgres's
        // does not
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {MEMORY_COLUMNS} FROM memories
             WHERE user_id = $1 AND (LOWER(content) LIKE $2 OR LOWER(tags) LIKE $2)
               AND deleted_at IS NULL
             ORDER BY pinned DESC, accessed_at DESC"
        )))
        .bind(user_id.to_string())
        .bind(format!("%{}%", query.to_lowercase()))
        .fetch_all(self.pool.pool())
        .await?;

        Ok(memories_from_rows(&rows))
    }

    /// Get memories for context injection (pinned + recent, up to `max_items`)
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn get_context(&self, user_id: &str, max_items: usize) -> Result<Vec<Memory>> {
        #[allow(clippy::cast_possible_wrap)]
        let limit = max_items as i64;
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {MEMORY_COLUMNS} FROM memories
             WHERE user_id = $1 AND deleted_at IS NULL
             ORDER BY pinned DESC, access_count DESC, accessed_at DESC LIMIT $2"
        )))
        .bind(user_id.to_string())
        .bind(limit)
        .fetch_all(self.pool.pool())
        .await?;

        Ok(memories_from_rows(&rows))
    }

    /// Search memories by vector similarity with temporal decay and MMR diversity re-ranking.
    ///
    /// Over-fetches 3x candidates, applies temporal decay to distance scores,
    /// then uses MMR to select diverse results before returning the top `limit`.
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn search_similar(
        &self,
        user_id: &str,
        embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<Memory>> {
        // Over-fetch 3x candidates for temporal decay + MMR re-ranking
        let fetch_limit = limit * 3;

        // Nearest vectors by L2 distance, joined back to memories to filter by user
        let prefixed_columns = MEMORY_COLUMNS
            .split(", ")
            .map(|c| format!("m.{c}"))
            .collect::<Vec<_>>()
            .join(", ");

        let nearest = match self.pool.backend() {
            Backend::Sqlite => {
                r"SELECT memory_id, distance
                  FROM memories_vec
                  WHERE embedding MATCH $1
                  ORDER BY distance
                  LIMIT $2"
            }
            Backend::Postgres => {
                r"SELECT memory_id, CAST(embedding <-> $1::vector AS DOUBLE PRECISION) AS distance
                  FROM memories_vec
                  ORDER BY embedding <-> $1::vector
                  LIMIT $2"
            }
        };
        let sql = format!(
            r"SELECT {prefixed_columns}, v.distance
              FROM memories m
              INNER JOIN ({nearest}) v ON m.id = v.memory_id
              WHERE m.user_id = $3 AND m.deleted_at IS NULL"
        );

        #[allow(clippy::cast_possible_wrap)]
        let rows = super::encode_embedding(self.pool.backend(), embedding)
            .bind(sqlx::query(sqlx::AssertSqlSafe(sql)))
            .bind(fetch_limit as i64)
            .bind(user_id.to_string())
            .fetch_all(self.pool.pool())
            .await?;

        let rows = rows.iter().filter_map(|row| {
            let memory_row = row_to_memory_row(row).ok()?;
            // distance is the 19th column (0-indexed 18)
            let distance: f64 = row.try_get(18).ok()?;
            Some((memory_row, distance))
        });

        let now = Utc::now();
        let candidates: Vec<MmrCandidate> = rows
            .map(|(row, distance)| {
                let memory = row.into_memory();
                let decay = temporal_decay_factor(&memory.accessed_at, &now);
                // Lower distance = better. Apply temporal decay as a bonus for recent memories.
                // Combined score: lower is better (subtract decay bonus)
                let combined = distance * DECAY_WEIGHT.mul_add(-decay, 1.0);
                // Invert so higher = better for MMR (MMR selects highest scores)
                let score = 1.0 / (1.0 + combined);
                MmrCandidate { memory, score }
            })
            .collect();

        Ok(mmr_rerank(candidates, limit, 0.7))
    }

    /// Search memories using BM25 keyword ranking
    ///
    /// Loads all non-deleted memories for the user, scores them with BM25,
    /// and returns the top matches. Falls back to LIKE search if BM25
    /// returns no results (handles partial/substring matches).
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn search_keyword(
        &self,
        user_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<Memory>> {
        let all = self.list(user_id, None).await?;

        if all.is_empty() {
            return Ok(Vec::new());
        }

        // Build searchable text for each memory: content + tags
        let documents: Vec<String> = all
            .iter()
            .map(|m| {
                let mut text = m.content.clone();
                if !m.tags.is_empty() {
                    text.push(' ');
                    text.push_str(&m.tags.join(" "));
                }
                text
            })
            .collect();

        let scorer = agent_core::knowledge::Bm25Scorer::new(&documents);
        let ranked = scorer.score(query);

        if ranked.is_empty() {
            // BM25 found nothing; fall back to LIKE for partial matches
            let mut fallback = self.search(user_id, query).await?;
            fallback.truncate(limit);
            return Ok(fallback);
        }

        let results: Vec<Memory> = ranked
            .into_iter()
            .take(limit)
            .map(|(idx, _score)| all[idx].clone())
            .collect();

        Ok(results)
    }

    /// Hybrid search combining BM25 keyword ranking with vector similarity
    ///
    /// When an embedding is provided, results from both BM25 keyword search
    /// and vector search are merged and deduplicated. Keyword matches are
    /// prioritized (appear first), followed by vector-similar results.
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn search_hybrid(
        &self,
        user_id: &str,
        query: &str,
        embedding: Option<&[f32]>,
        limit: usize,
    ) -> Result<Vec<Memory>> {
        // Start with BM25-ranked keyword results
        let mut results = self.search_keyword(user_id, query, limit).await?;
        let mut seen: std::collections::HashSet<String> =
            results.iter().map(|m| m.id.clone()).collect();

        // Merge in vector search results if embedding available
        if let Some(emb) = embedding {
            let similar = self.search_similar(user_id, emb, limit).await?;
            for mem in similar {
                if seen.insert(mem.id.clone()) {
                    results.push(mem);
                }
            }
        }

        // Truncate to limit
        results.truncate(limit);
        Ok(results)
    }

    /// Soft-delete a memory (sets `deleted_at` tombstone)
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn delete(&self, id: &str) -> Result<bool> {
        let mut tx = self.pool.pool().begin().await?;

        // Delete from vector table (no longer needed for search)
        sqlx::query("DELETE FROM memories_vec WHERE memory_id = $1")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;

        // Soft-delete: set tombstone instead of removing the row
        let now = super::now();
        let deleted = sqlx::query(
            "UPDATE memories SET deleted_at = $1, updated_at = $1 WHERE id = $2 AND deleted_at IS NULL",
        )
        .bind(now)
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?
        .rows_affected();

        tx.commit().await?;
        Ok(deleted > 0)
    }

    /// Hard-delete memories with tombstones older than the given cutoff
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn purge_tombstones(&self, cutoff_days: u32) -> Result<usize> {
        let cutoff =
            super::format_timestamp(Utc::now() - chrono::Duration::days(i64::from(cutoff_days)));

        let deleted =
            sqlx::query("DELETE FROM memories WHERE deleted_at IS NOT NULL AND deleted_at < $1")
                .bind(cutoff)
                .execute(self.pool.pool())
                .await?
                .rows_affected();

        if deleted > 0 {
            tracing::info!(count = deleted, cutoff_days, "purged memory tombstones");
        }

        Ok(usize::try_from(deleted).unwrap_or(usize::MAX))
    }

    /// Update a memory's embedding
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn set_embedding(&self, id: &str, embedding: &[f32]) -> Result<bool> {
        let mut tx = self.pool.pool().begin().await?;

        // Update in memories table
        let updated = sqlx::query("UPDATE memories SET embedding = $1 WHERE id = $2")
            .bind(super::embedder::Embedder::to_bytes(embedding))
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?
            .rows_affected();

        if updated == 0 {
            return Ok(false);
        }

        // Upsert in vector table
        upsert_vector(&mut tx, self.pool.backend(), id, embedding).await?;

        tx.commit().await?;
        Ok(true)
    }

    /// Update a memory's content or pinned status
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn update(
        &self,
        id: &str,
        content: Option<&str>,
        pinned: Option<bool>,
    ) -> Result<bool> {
        if content.is_none() && pinned.is_none() {
            return Ok(false);
        }

        // Unchanged fields keep their value; content_hash follows content
        let rows_affected = sqlx::query(
            "UPDATE memories SET
                content = COALESCE($1, content),
                content_hash = COALESCE($2, content_hash),
                pinned = COALESCE($3, pinned),
                updated_at = $4
             WHERE id = $5",
        )
        .bind(content.map(str::to_string))
        .bind(content.map(Memory::compute_content_hash))
        .bind(pinned.map(i64::from))
        .bind(super::now())
        .bind(id.to_string())
        .execute(self.pool.pool())
        .await?
        .rows_affected();

        Ok(rows_affected > 0)
    }

    /// Query memories that need syncing (`updated_at` > `synced_at` or never synced)
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn unsynced(&self) -> Result<Vec<Memory>> {
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {MEMORY_COLUMNS} FROM memories
             WHERE synced_at IS NULL OR updated_at > synced_at
             ORDER BY updated_at ASC"
        )))
        .fetch_all(self.pool.pool())
        .await?;

        Ok(memories_from_rows(&rows))
    }

    /// Mark memories as synced
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn mark_synced(&self, ids: &[&str]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }

        let placeholders: Vec<String> = (2..=ids.len() + 1).map(|i| format!("${i}")).collect();
        let sql = format!(
            "UPDATE memories SET synced_at = $1 WHERE id IN ({})",
            placeholders.join(", ")
        );

        let mut query = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(super::now());
        for id in ids {
            query = query.bind((*id).to_string());
        }
        query.execute(self.pool.pool()).await?;

        Ok(())
    }

    /// Set the `cloud_id` for a memory after successful push
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn set_cloud_id(&self, id: &str, cloud_id: &str) -> Result<bool> {
        let updated = sqlx::query("UPDATE memories SET cloud_id = $1 WHERE id = $2")
            .bind(cloud_id.to_string())
            .bind(id.to_string())
            .execute(self.pool.pool())
            .await?
            .rows_affected();

        Ok(updated > 0)
    }

    /// Upsert a memory from a remote sync (used during pull)
    ///
    /// Uses `content_hash` for dedup: if a memory with the same `user_id` and
    /// `content_hash` exists, apply LWW merge. Otherwise insert as new.
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn upsert_from_remote(&self, memory: &Memory) -> Result<bool> {
        let tags_json = serde_json::to_string(&memory.tags).unwrap_or_else(|_| "[]".to_string());
        let mut tx = self.pool.pool().begin().await?;

        // Check for existing memory by content_hash (dedup)
        if let Some(ref content_hash) = memory.content_hash {
            let existing = sqlx::query(
                "SELECT id, COALESCE(updated_at, created_at), access_count FROM memories
                 WHERE user_id = $1 AND content_hash = $2",
            )
            .bind(memory.user_id.clone())
            .bind(content_hash.clone())
            .fetch_optional(&mut *tx)
            .await?;

            if let Some(row) = existing {
                let existing_id: String = row.try_get(0)?;
                let existing_updated: String = row.try_get(1)?;
                let existing_count = u32::try_from(row.try_get::<i64, _>(2)?).unwrap_or(0);
                let merged_count = std::cmp::max(memory.access_count, existing_count);

                // LWW merge: only update if remote is newer
                if memory.updated_at > existing_updated {
                    sqlx::query(
                        r"UPDATE memories SET
                            content = $1, tags = $2, pinned = $3, access_count = $4,
                            updated_at = $5, deleted_at = $6, cloud_id = $7, synced_at = $8
                          WHERE id = $9",
                    )
                    .bind(memory.content.clone())
                    .bind(tags_json)
                    .bind(i64::from(memory.pinned))
                    .bind(i64::from(merged_count))
                    .bind(memory.updated_at.clone())
                    .bind(memory.deleted_at.clone())
                    .bind(memory.cloud_id.clone())
                    .bind(super::now())
                    .bind(existing_id)
                    .execute(&mut *tx)
                    .await?;

                    tx.commit().await?;
                    return Ok(true);
                }

                // Remote is older, only merge access_count
                if merged_count > existing_count {
                    sqlx::query(
                        "UPDATE memories SET access_count = $1, synced_at = $2 WHERE id = $3",
                    )
                    .bind(i64::from(merged_count))
                    .bind(super::now())
                    .bind(existing_id)
                    .execute(&mut *tx)
                    .await?;
                }

                tx.commit().await?;
                return Ok(false);
            }
        }

        // No existing match, insert as new
        Self::insert(&mut tx, self.pool.backend(), memory).await?;

        // Mark as synced immediately since it came from the cloud
        sqlx::query("UPDATE memories SET synced_at = $1 WHERE id = $2")
            .bind(super::now())
            .bind(memory.id.clone())
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(true)
    }

    /// Check if a memory with the given content hash already exists for a user
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn exists_by_content_hash(&self, user_id: &str, content_hash: &str) -> Result<bool> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM memories WHERE user_id = $1 AND content_hash = $2 AND deleted_at IS NULL",
        )
        .bind(user_id.to_string())
        .bind(content_hash.to_string())
        .fetch_one(self.pool.pool())
        .await?;

        Ok(count > 0)
    }

    /// Get exportable memories: pinned + top by access count
    ///
    /// # Errors
    ///
    /// Returns error if database operation fails
    pub async fn get_exportable(&self, user_id: &str, max_items: usize) -> Result<Vec<Memory>> {
        #[allow(clippy::cast_possible_wrap)]
        let limit = max_items as i64;
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {MEMORY_COLUMNS} FROM memories
             WHERE user_id = $1 AND deleted_at IS NULL
             ORDER BY pinned DESC, access_count DESC, accessed_at DESC LIMIT $2"
        )))
        .bind(user_id.to_string())
        .bind(limit)
        .fetch_all(self.pool.pool())
        .await?;

        Ok(memories_from_rows(&rows))
    }

    /// Format memories for prompt injection
    #[must_use]
    pub fn format_for_prompt(memories: &[Memory]) -> String {
        use std::fmt::Write;

        if memories.is_empty() {
            return String::new();
        }

        let mut output = String::from("Remembered facts about you:\n");

        for mem in memories {
            let _ = writeln!(output, "- [{}] {}", mem.category, mem.content);
        }

        output
    }
}

/// Implement agent-core's `MemoryStore` trait for shared interface compatibility
///
/// Maps between Beacon's rich `Memory`/`MemoryCategory` types and agent-core's
/// `MemoryItem`/`MemoryCategory`. The underlying storage remains Beacon's
/// SQLite+sqlite-vec hybrid search backend.
#[async_trait::async_trait]
impl agent_core::memory::MemoryStore for MemoryRepo {
    async fn add(&self, item: agent_core::memory::MemoryItem) -> anyhow::Result<String> {
        let category = match item.category {
            agent_core::memory::MemoryCategory::Preference => MemoryCategory::Preference,
            agent_core::memory::MemoryCategory::Fact => MemoryCategory::Fact,
            agent_core::memory::MemoryCategory::Correction => MemoryCategory::Correction,
            agent_core::memory::MemoryCategory::General => MemoryCategory::General,
        };

        let user_id = item.user_id.unwrap_or_else(|| "default".to_string());
        let mut memory = Memory::new(user_id, category, item.content);

        if item.pinned {
            memory = memory.pinned();
        }
        for tag in &item.tags {
            memory = memory.with_tag(tag);
        }
        if let Some(embedding) = item.embedding {
            memory = memory.with_embedding(embedding);
        }

        let id = memory.id.clone();
        Self::add(self, &memory).await?;
        Ok(id)
    }

    async fn get(&self, id: &str) -> anyhow::Result<Option<agent_core::memory::MemoryItem>> {
        Ok(Self::get(self, id).await?.map(to_core_item))
    }

    async fn list(
        &self,
        category: Option<agent_core::memory::MemoryCategory>,
    ) -> anyhow::Result<Vec<agent_core::memory::MemoryItem>> {
        let beacon_cat = category.as_ref().map(to_beacon_category);
        // Use "default" user_id since trait doesn't pass one
        let memories = Self::list(self, "default", beacon_cat).await?;
        Ok(memories.into_iter().map(to_core_item).collect())
    }

    async fn search(
        &self,
        query: &str,
        limit: Option<usize>,
    ) -> anyhow::Result<Vec<agent_core::memory::MemoryItem>> {
        let memories = self
            .search_keyword("default", query, limit.unwrap_or(10))
            .await?;
        Ok(memories.into_iter().map(to_core_item).collect())
    }

    async fn delete(&self, id: &str) -> anyhow::Result<bool> {
        Ok(Self::delete(self, id).await?)
    }

    async fn update(
        &self,
        id: &str,
        content: Option<String>,
        pinned: Option<bool>,
    ) -> anyhow::Result<()> {
        Self::update(self, id, content.as_deref(), pinned).await?;
        Ok(())
    }

    async fn get_context(
        &self,
        max_items: usize,
    ) -> anyhow::Result<Vec<agent_core::memory::MemoryItem>> {
        let memories = Self::get_context(self, "default", max_items).await?;
        Ok(memories.into_iter().map(to_core_item).collect())
    }
}

/// Convert a Beacon `Memory` to agent-core `MemoryItem`
fn to_core_item(memory: Memory) -> agent_core::memory::MemoryItem {
    let category = match memory.category {
        MemoryCategory::Preference => agent_core::memory::MemoryCategory::Preference,
        MemoryCategory::Fact => agent_core::memory::MemoryCategory::Fact,
        MemoryCategory::Correction => agent_core::memory::MemoryCategory::Correction,
        MemoryCategory::General => agent_core::memory::MemoryCategory::General,
    };

    let mut item = agent_core::memory::MemoryItem::new(memory.content, category);
    // Preserve the original ID instead of generating a new one
    item.id = memory.id;
    item.created_at = memory.created_at;
    item.accessed_at = memory.accessed_at;
    item.access_count = memory.access_count;
    item.tags = memory.tags;
    item.pinned = memory.pinned;
    item.embedding = memory.embedding;
    item.user_id = Some(memory.user_id);
    item
}

/// Convert an agent-core `MemoryCategory` to Beacon's `MemoryCategory`
const fn to_beacon_category(cat: &agent_core::memory::MemoryCategory) -> MemoryCategory {
    match cat {
        agent_core::memory::MemoryCategory::Preference => MemoryCategory::Preference,
        agent_core::memory::MemoryCategory::Fact => MemoryCategory::Fact,
        agent_core::memory::MemoryCategory::Correction => MemoryCategory::Correction,
        agent_core::memory::MemoryCategory::General => MemoryCategory::General,
    }
}

/// Internal struct for database row mapping
struct MemoryRow {
    id: String,
    user_id: String,
    category: String,
    content: String,
    tags: String,
    pinned: i64,
    access_count: i64,
    created_at: String,
    accessed_at: String,
    embedding: Option<Vec<u8>>,
    source_session_id: Option<String>,
    source_channel: Option<String>,
    content_hash: Option<String>,
    origin_device_id: Option<String>,
    updated_at: Option<String>,
    deleted_at: Option<String>,
    synced_at: Option<String>,
    cloud_id: Option<String>,
}

impl MemoryRow {
    fn into_memory(self) -> Memory {
        let now_str = Utc::now().to_rfc3339();
        Memory {
            id: self.id,
            user_id: self.user_id,
            category: MemoryCategory::from_str_value(&self.category)
                .unwrap_or(MemoryCategory::General),
            content: self.content,
            tags: serde_json::from_str(&self.tags).unwrap_or_default(),
            pinned: self.pinned != 0,
            access_count: u32::try_from(self.access_count).unwrap_or(0),
            created_at: DateTime::parse_from_rfc3339(&self.created_at)
                .map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc)),
            accessed_at: DateTime::parse_from_rfc3339(&self.accessed_at)
                .map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc)),
            embedding: self
                .embedding
                .map(|b| super::embedder::Embedder::from_bytes(&b)),
            source_session_id: self.source_session_id,
            source_channel: self.source_channel,
            content_hash: self.content_hash,
            origin_device_id: self.origin_device_id,
            updated_at: self.updated_at.unwrap_or(now_str),
            deleted_at: self.deleted_at,
            synced_at: self.synced_at,
            cloud_id: self.cloud_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[tokio::test]
    async fn test_memory_crud() {
        let pool = db::init_memory().await.unwrap();
        let repo = MemoryRepo::new(pool);

        // Create a user first
        let user_repo = crate::db::UserRepo::new(repo.pool.clone());
        let user = user_repo.find_or_create("test_user").await.unwrap();

        // Add a memory
        let memory = Memory::new(
            user.id.clone(),
            MemoryCategory::Preference,
            "User prefers dark mode".to_string(),
        );
        repo.add(&memory).await.unwrap();

        // Get the memory
        let fetched = repo.get(&memory.id).await.unwrap().unwrap();
        assert_eq!(fetched.content, "User prefers dark mode");
        assert_eq!(fetched.category, MemoryCategory::Preference);

        // List memories
        let memories = repo.list(&user.id, None).await.unwrap();
        assert_eq!(memories.len(), 1);

        // Search
        let found = repo.search(&user.id, "dark mode").await.unwrap();
        assert_eq!(found.len(), 1);

        // Delete
        assert!(repo.delete(&memory.id).await.unwrap());
        assert!(repo.get(&memory.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_memory_context() {
        let pool = db::init_memory().await.unwrap();
        let repo = MemoryRepo::new(pool);

        let user_repo = crate::db::UserRepo::new(repo.pool.clone());
        let user = user_repo.find_or_create("context_user").await.unwrap();

        // Add some memories
        let m1 = Memory::new(
            user.id.clone(),
            MemoryCategory::Preference,
            "Prefers vim".to_string(),
        );
        let m2 = Memory::new(
            user.id.clone(),
            MemoryCategory::Fact,
            "Lives in Seattle".to_string(),
        )
        .pinned();
        repo.add(&m1).await.unwrap();
        repo.add(&m2).await.unwrap();

        // Get context (pinned should come first)
        let context = repo.get_context(&user.id, 10).await.unwrap();
        assert_eq!(context.len(), 2);
        assert!(context[0].pinned); // Pinned first
    }

    #[tokio::test]
    async fn test_search_hybrid_text_only() {
        let pool = db::init_memory().await.unwrap();
        let repo = MemoryRepo::new(pool);

        let user_repo = crate::db::UserRepo::new(repo.pool.clone());
        let user = user_repo.find_or_create("hybrid_user").await.unwrap();

        let m1 = Memory::new(
            user.id.clone(),
            MemoryCategory::Preference,
            "Likes dark mode".to_string(),
        );
        let m2 = Memory::new(
            user.id.clone(),
            MemoryCategory::Fact,
            "Works at Acme Corp".to_string(),
        );
        repo.add(&m1).await.unwrap();
        repo.add(&m2).await.unwrap();

        // Text-only hybrid search (no embedding)
        let results = repo
            .search_hybrid(&user.id, "dark", None, 10)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "Likes dark mode");
    }

    #[tokio::test]
    async fn temporal_decay_recent_is_high() {
        let now = Utc::now();
        let factor = super::temporal_decay_factor(&now, &now);
        assert!(factor > 0.99, "factor for now should be ~1.0, got {factor}");
    }

    #[tokio::test]
    async fn temporal_decay_old_is_low() {
        let now = Utc::now();
        let old = now - chrono::Duration::days(30);
        let factor = super::temporal_decay_factor(&old, &now);
        assert!(
            factor < 0.1,
            "factor for 30 days ago should be < 0.1, got {factor}"
        );
    }

    #[tokio::test]
    async fn temporal_decay_half_life() {
        let now = Utc::now();
        let half = now - chrono::Duration::days(7);
        let factor = super::temporal_decay_factor(&half, &now);
        assert!(
            (factor - 0.5).abs() < 0.01,
            "factor at 7 days should be ~0.5, got {factor}"
        );
    }

    #[tokio::test]
    async fn cosine_similarity_identical_is_one() {
        let v = vec![1.0_f32, 2.0, 3.0];
        let sim = super::cosine_similarity(&v, &v);
        assert!(
            (sim - 1.0).abs() < 0.001,
            "identical vectors should have sim ~1.0, got {sim}"
        );
    }

    #[tokio::test]
    async fn cosine_similarity_orthogonal_is_zero() {
        let a = vec![1.0_f32, 0.0, 0.0];
        let b = vec![0.0_f32, 1.0, 0.0];
        let sim = super::cosine_similarity(&a, &b);
        assert!(
            sim.abs() < 0.001,
            "orthogonal vectors should have sim ~0.0, got {sim}"
        );
    }

    #[tokio::test]
    async fn mmr_reduces_duplicate_results() {
        // Create 3 candidates: 2 near-identical + 1 distinct
        let emb_a = vec![1.0_f32; 8];
        let emb_a2 = {
            let mut v = vec![1.0_f32; 8];
            v[0] = 0.99; // Nearly identical to a
            v
        };
        let emb_b = vec![0.0_f32, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]; // Orthogonal

        let candidates = vec![
            super::MmrCandidate {
                memory: Memory::new(
                    "u1".to_string(),
                    MemoryCategory::Fact,
                    "Dark mode".to_string(),
                )
                .with_embedding(emb_a),
                score: 0.9,
            },
            super::MmrCandidate {
                memory: Memory::new(
                    "u1".to_string(),
                    MemoryCategory::Fact,
                    "Dark theme".to_string(),
                )
                .with_embedding(emb_a2),
                score: 0.9,
            },
            super::MmrCandidate {
                memory: Memory::new(
                    "u1".to_string(),
                    MemoryCategory::Fact,
                    "Works at Acme".to_string(),
                )
                .with_embedding(emb_b),
                score: 0.9,
            },
        ];

        // With equal relevance scores, MMR should prefer diversity
        let results = super::mmr_rerank(candidates, 2, 0.5);
        assert_eq!(results.len(), 2);
        // Both groups should be represented
        let contents: Vec<&str> = results.iter().map(|m| m.content.as_str()).collect();
        assert!(
            contents.contains(&"Works at Acme"),
            "diverse result should be included: {contents:?}"
        );
    }

    #[tokio::test]
    async fn test_format_for_prompt() {
        let memories = vec![
            Memory::new(
                "u1".to_string(),
                MemoryCategory::Preference,
                "Uses vim".to_string(),
            ),
            Memory::new(
                "u1".to_string(),
                MemoryCategory::Fact,
                "Works at Acme".to_string(),
            ),
        ];

        let formatted = MemoryRepo::format_for_prompt(&memories);
        assert!(formatted.contains("[preference] Uses vim"));
        assert!(formatted.contains("[fact] Works at Acme"));
    }
}
