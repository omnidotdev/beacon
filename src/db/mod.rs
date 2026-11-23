//! Storage: one async SQL layer over `SQLite` and Postgres
//!
//! A self-hosted gateway keeps everything in a single local `SQLite` file; the
//! cloud gateway uses Postgres, which survives deploys and is backed up with
//! the rest of Beacon's data. Both run the same repository code through sqlx's
//! `Any` driver, so queries are written once in portable SQL:
//!
//! - placeholders are `$1`, `$2`, ... (both backends bind them by position)
//! - timestamps are `TEXT` in `YYYY-MM-DD HH:MM:SS` UTC, generated in Rust by
//!   [`now`] rather than by a backend function, so they sort and compare the
//!   same everywhere
//! - integers are 64-bit and booleans are stored as integers
//!
//! Only vector search (`sqlite-vec` versus pgvector) and schema migrations
//! differ per backend; see [`Backend`] and the `schema` module

pub mod billing_workspace;
pub mod embedder;
pub mod indexer;
pub mod knowledge;
pub mod memory;
pub mod memory_ai;
pub mod persona;
mod schema;
pub mod session;
pub mod skill;
pub mod telegram;
pub mod usage;
pub mod user;

use std::path::Path;
use std::sync::Once;
use std::time::Duration;

use sqlx::AnyPool;
use sqlx::any::AnyPoolOptions;

use crate::{Error, Result};

pub use billing_workspace::BillingWorkspaceRepo;
pub use embedder::{EMBEDDING_DIM, Embedder};
pub use indexer::{ExtractedFact, ExtractionResponse, Indexer};
pub use knowledge::{KnowledgePackRepo, KnowledgePackRow};
pub use memory::{Memory, MemoryCategory, MemoryRepo};
pub use memory_ai::{MemoryAi, MemoryAiError};
pub use persona::{InstalledPersona, PersonaRepo};
pub use schema::SCHEMA_VERSION;
pub use session::{Message, MessageRole, Session, SessionRepo};
pub use skill::SkillRepo;
pub use telegram::{TelegramGroupConfig, TelegramGroupConfigRepo};
pub use usage::{UsageRecord, UsageRepo};
pub use user::{User, UserContext, UserRepo};

/// Environment variable naming a Postgres database for the test suite. When
/// set, [`init_memory`] gives each test its own throwaway schema there, so the
/// whole suite runs against Postgres as well as `SQLite`
pub const TEST_DATABASE_URL_ENV: &str = "BEACON_TEST_DATABASE_URL";

/// Which SQL backend a pool talks to
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// A local `SQLite` file (self-hosted)
    Sqlite,
    /// A Postgres server (cloud)
    Postgres,
}

/// Database connection pool, cheap to clone
#[derive(Clone, Debug)]
pub struct DbPool {
    pool: AnyPool,
    backend: Backend,
}

impl DbPool {
    /// The underlying sqlx pool, for running queries
    #[must_use]
    pub const fn pool(&self) -> &AnyPool {
        &self.pool
    }

    /// Which backend this pool talks to
    #[must_use]
    pub const fn backend(&self) -> Backend {
        self.backend
    }

    /// Whether this pool talks to Postgres
    #[must_use]
    pub const fn is_postgres(&self) -> bool {
        matches!(self.backend, Backend::Postgres)
    }
}

/// The current time as a stored timestamp (`YYYY-MM-DD HH:MM:SS`, UTC)
#[must_use]
pub fn now() -> String {
    format_timestamp(chrono::Utc::now())
}

/// Format a time as a stored timestamp (`YYYY-MM-DD HH:MM:SS`, UTC)
#[must_use]
pub fn format_timestamp(at: chrono::DateTime<chrono::Utc>) -> String {
    at.format("%Y-%m-%d %H:%M:%S").to_string()
}

static SQLITE_VEC_INIT: Once = Once::new();

/// Register the sqlite-vec extension for every `SQLite` connection opened from
/// now on. Safe to call repeatedly; only the first call has an effect
#[allow(unsafe_code)]
pub(crate) fn register_sqlite_vec() {
    SQLITE_VEC_INIT.call_once(|| {
        // SAFETY: `sqlite3_vec_init` is sqlite-vec's extension entry point, made
        // to be passed to `sqlite3_auto_extension`. The transmute only restates
        // the function pointer with the signature that registration API expects
        unsafe {
            libsqlite3_sys::sqlite3_auto_extension(Some(std::mem::transmute::<
                *const (),
                unsafe extern "C" fn(
                    *mut libsqlite3_sys::sqlite3,
                    *mut *mut i8,
                    *const libsqlite3_sys::sqlite3_api_routines,
                ) -> i32,
            >(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }
    });
}

/// Connect to a database by URL (`sqlite:<path>` or `postgres://...`) and bring
/// its schema up to date
///
/// # Errors
///
/// Returns error if the database cannot be reached or migrated
pub async fn connect(url: &str) -> Result<DbPool> {
    let backend = backend_for(url)?;
    // A local SQLite file serializes writes anyway; Postgres serves every
    // signed-in user of the cloud gateway at once
    let max_connections = match backend {
        Backend::Sqlite => 4,
        Backend::Postgres => 10,
    };
    let db = open(url, backend, max_connections, None).await?;
    schema::init(&db).await?;

    tracing::info!(
        version = SCHEMA_VERSION,
        backend = ?backend,
        "database initialized"
    );
    Ok(db)
}

/// Open (creating if needed) a local `SQLite` database file
///
/// # Errors
///
/// Returns error if the database cannot be opened or initialized
pub async fn init<P: AsRef<Path>>(path: P) -> Result<DbPool> {
    connect(&format!("sqlite://{}?mode=rwc", path.as_ref().display())).await
}

/// A fresh, empty database for tests: in-memory `SQLite`, or when
/// [`TEST_DATABASE_URL_ENV`] is set, a new schema in that Postgres database
///
/// # Errors
///
/// Returns error if the database cannot be initialized
pub async fn init_memory() -> Result<DbPool> {
    let db = match std::env::var(TEST_DATABASE_URL_ENV) {
        Ok(url) if !url.is_empty() => open_test_schema(&url).await?,
        // One long-lived connection: every `:memory:` connection is its own db
        _ => open("sqlite::memory:", Backend::Sqlite, 1, None).await?,
    };
    schema::init(&db).await?;
    Ok(db)
}

/// Create an isolated schema in a Postgres test database and connect with it
/// first on the search path (pgvector's types stay reachable via `public`)
async fn open_test_schema(url: &str) -> Result<DbPool> {
    let name = format!("test_{}", uuid::Uuid::new_v4().simple());
    let admin = open(url, Backend::Postgres, 1, None).await?;
    sqlx::query("CREATE EXTENSION IF NOT EXISTS vector")
        .execute(admin.pool())
        .await?;
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {name}")))
        .execute(admin.pool())
        .await?;
    admin.pool().close().await;

    open(url, Backend::Postgres, 2, Some(name)).await
}

fn backend_for(url: &str) -> Result<Backend> {
    if url.starts_with("sqlite:") {
        Ok(Backend::Sqlite)
    } else if url.starts_with("postgres://") || url.starts_with("postgresql://") {
        Ok(Backend::Postgres)
    } else {
        Err(Error::Database(
            "database URL must start with sqlite: or postgres://".to_string(),
        ))
    }
}

async fn open(
    url: &str,
    backend: Backend,
    max_connections: u32,
    search_path: Option<String>,
) -> Result<DbPool> {
    sqlx::any::install_default_drivers();
    if backend == Backend::Sqlite {
        register_sqlite_vec();
    }

    let mut options = AnyPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(Duration::from_secs(30));

    // An in-memory database lives only as long as its connection
    if url.contains(":memory:") {
        options = options
            .min_connections(1)
            .idle_timeout(None)
            .max_lifetime(None);
    }

    if let Some(schema) = search_path {
        options = options.after_connect(move |conn, _meta| {
            let statement = format!("SET search_path TO {schema}, public");
            Box::pin(async move {
                sqlx::query(sqlx::AssertSqlSafe(statement))
                    .execute(&mut *conn)
                    .await?;
                Ok(())
            })
        });
    }

    let pool = options
        .connect(url)
        .await
        .map_err(|e| Error::Database(format!("failed to open database: {e}")))?;

    Ok(DbPool { pool, backend })
}

/// Open an existing local `SQLite` file read-only, without migrating it, for
/// maintenance (backups, integrity checks)
async fn open_sqlite_read_only(path: &Path) -> Result<DbPool> {
    open(
        &format!("sqlite://{}?mode=ro", path.display()),
        Backend::Sqlite,
        1,
        None,
    )
    .await
}

/// Write a consistent snapshot of a local `SQLite` database to `target`
/// (`VACUUM INTO`), safe while the gateway is running. `target` must not exist
/// or must be empty
///
/// # Errors
///
/// Returns error if the database cannot be read or the snapshot written
pub async fn snapshot_sqlite(source: &Path, target: &Path) -> Result<()> {
    let db = open_sqlite_read_only(source).await?;
    sqlx::query("VACUUM INTO $1")
        .bind(target.display().to_string())
        .execute(db.pool())
        .await?;
    db.pool().close().await;
    Ok(())
}

/// Run `SQLite`'s integrity check on a local database file, returning its
/// verdict (`ok` when healthy)
///
/// # Errors
///
/// Returns error if the database cannot be opened or checked
pub async fn sqlite_integrity_check(path: &Path) -> Result<String> {
    let db = open_sqlite_read_only(path).await?;
    let verdict: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(db.pool())
        .await?;
    db.pool().close().await;
    Ok(verdict)
}

/// Encode an embedding for a vector column bind: raw little-endian `f32`
/// bytes for sqlite-vec, the `[a,b,...]` text literal for pgvector
#[must_use]
pub fn encode_embedding(backend: Backend, embedding: &[f32]) -> EmbeddingParam {
    match backend {
        Backend::Sqlite => {
            EmbeddingParam::Bytes(embedding.iter().flat_map(|f| f.to_le_bytes()).collect())
        }
        Backend::Postgres => EmbeddingParam::Text(format!(
            "[{}]",
            embedding
                .iter()
                .map(f32::to_string)
                .collect::<Vec<_>>()
                .join(",")
        )),
    }
}

/// An embedding encoded for its backend (see [`encode_embedding`])
#[derive(Debug, Clone)]
pub enum EmbeddingParam {
    /// sqlite-vec blob
    Bytes(Vec<u8>),
    /// pgvector text literal, bound as `$n::vector`
    Text(String),
}

impl EmbeddingParam {
    /// Bind onto a query in the right representation for its backend
    pub fn bind(
        self,
        query: sqlx::query::Query<'_, sqlx::Any, sqlx::any::AnyArguments>,
    ) -> sqlx::query::Query<'_, sqlx::Any, sqlx::any::AnyArguments> {
        match self {
            Self::Bytes(bytes) => query.bind(bytes),
            Self::Text(text) => query.bind(text),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_init_memory() {
        let db = init_memory().await.unwrap();
        let one: i64 = sqlx::query_scalar("SELECT 1")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(one, 1);
    }

    #[test]
    fn timestamps_match_the_stored_format() {
        let at = chrono::DateTime::parse_from_rfc3339("2026-11-23T08:05:09.123Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert_eq!(format_timestamp(at), "2026-11-23 08:05:09");
    }

    #[test]
    fn encodes_embeddings_per_backend() {
        match encode_embedding(Backend::Sqlite, &[1.0, 0.5]) {
            EmbeddingParam::Bytes(bytes) => assert_eq!(bytes.len(), 8),
            EmbeddingParam::Text(_) => panic!("sqlite takes bytes"),
        }
        match encode_embedding(Backend::Postgres, &[1.0, 0.5]) {
            EmbeddingParam::Text(text) => assert_eq!(text, "[1,0.5]"),
            EmbeddingParam::Bytes(_) => panic!("postgres takes text"),
        }
    }
}
