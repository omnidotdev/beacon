//! Database schema and migrations
//!
//! `SQLite` (self-hosted) keeps its incremental migrations, so an existing
//! local database upgrades in place, tracking its version in `PRAGMA
//! user_version`. Postgres (cloud) starts from a baseline equal to the latest
//! `SQLite` schema and tracks versions in `schema_migrations`. A new migration
//! is written for both backends with the same version number; the parity test
//! (`tests/schema_parity.rs`) fails if their tables or columns drift apart

use sqlx::{AnyConnection, Row};

use super::{Backend, DbPool};
use crate::Result;

/// Current schema version
pub const SCHEMA_VERSION: i32 = 22;

/// Bring the database schema up to [`SCHEMA_VERSION`]
///
/// # Errors
///
/// Returns error if a migration fails
pub async fn init(db: &DbPool) -> Result<()> {
    let mut conn = db.pool().acquire().await?;
    match db.backend() {
        Backend::Sqlite => init_sqlite(&mut conn).await,
        Backend::Postgres => init_postgres(&mut conn).await,
    }
}

async fn init_sqlite(conn: &mut AnyConnection) -> Result<()> {
    let version: i64 = sqlx::query("PRAGMA user_version")
        .fetch_optional(&mut *conn)
        .await?
        .map_or(Ok(0), |row| row.try_get(0))?;

    if version < 1 {
        migrate_v1(conn).await?;
    }
    if version < 2 {
        migrate_v2(conn).await?;
    }
    if version < 3 {
        migrate_v3(conn).await?;
    }
    if version < 4 {
        migrate_v4(conn).await?;
    }
    if version < 5 {
        migrate_v5(conn).await?;
    }
    if version < 6 {
        migrate_v6(conn).await?;
    }
    if version < 7 {
        migrate_v7(conn).await?;
    }
    if version < 8 {
        migrate_v8(conn).await?;
    }
    if version < 9 {
        migrate_v9(conn).await?;
    }
    if version < 10 {
        migrate_v10(conn).await?;
    }
    if version < 11 {
        migrate_v11(conn).await?;
    }
    if version < 12 {
        migrate_v12(conn).await?;
    }
    if version < 13 {
        migrate_v13(conn).await?;
    }
    if version < 14 {
        migrate_v14(conn).await?;
    }
    if version < 15 {
        migrate_v15(conn).await?;
    }
    if version < 16 {
        migrate_v16(conn).await?;
    }
    if version < 17 {
        migrate_v17(conn).await?;
    }
    if version < 18 {
        migrate_v18(conn).await?;
    }
    if version < 19 {
        migrate_v19(conn).await?;
    }
    if version < 20 {
        migrate_v20(conn).await?;
    }
    if version < 21 {
        migrate_v21(conn).await?;
    }
    if version < 22 {
        migrate_v22(conn).await?;
    }

    Ok(())
}

async fn migrate_v1(conn: &mut AnyConnection) -> Result<()> {
    sqlx::raw_sql(
        r"
        -- Users table
        CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY,
            life_json_path TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        -- Sessions table
        CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL REFERENCES users(id),
            channel TEXT NOT NULL,
            channel_id TEXT NOT NULL,
            persona_id TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_sessions_user ON sessions(user_id);
        CREATE INDEX IF NOT EXISTS idx_sessions_channel ON sessions(channel, channel_id);

        -- Messages table
        CREATE TABLE IF NOT EXISTS messages (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL REFERENCES sessions(id),
            role TEXT NOT NULL CHECK(role IN ('user', 'assistant', 'system')),
            content TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id);

        -- User context (learned preferences)
        CREATE TABLE IF NOT EXISTS user_context (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL REFERENCES users(id),
            key TEXT NOT NULL,
            value TEXT NOT NULL,
            source TEXT NOT NULL DEFAULT 'learned',
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(user_id, key)
        );

        CREATE INDEX IF NOT EXISTS idx_user_context_user ON user_context(user_id);

        PRAGMA user_version = 1;
        ",
    )
    .execute(&mut *conn)
    .await?;

    tracing::info!("migrated to schema v1");
    Ok(())
}

async fn migrate_v2(conn: &mut AnyConnection) -> Result<()> {
    sqlx::raw_sql(r"
        -- Memories table for long-term memory storage
        CREATE TABLE IF NOT EXISTS memories (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL REFERENCES users(id),
            category TEXT NOT NULL CHECK(category IN ('preference', 'fact', 'correction', 'general')),
            content TEXT NOT NULL,
            tags TEXT NOT NULL DEFAULT '[]',
            pinned INTEGER NOT NULL DEFAULT 0,
            access_count INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            accessed_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_memories_user ON memories(user_id);
        CREATE INDEX IF NOT EXISTS idx_memories_category ON memories(category);
        CREATE INDEX IF NOT EXISTS idx_memories_pinned ON memories(pinned);

        PRAGMA user_version = 2;
        ")
        .execute(&mut *conn)
        .await?;

    tracing::info!("migrated to schema v2");
    Ok(())
}

async fn migrate_v3(conn: &mut AnyConnection) -> Result<()> {
    sqlx::raw_sql(r"
        -- Installed skills table
        CREATE TABLE IF NOT EXISTS installed_skills (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            description TEXT NOT NULL,
            version TEXT,
            author TEXT,
            tags TEXT NOT NULL DEFAULT '[]',
            permissions TEXT NOT NULL DEFAULT '[]',
            content TEXT NOT NULL,
            source_type TEXT NOT NULL CHECK(source_type IN ('local', 'manifold', 'bundled', 'plugin')),
            source_namespace TEXT,
            source_repository TEXT,
            enabled INTEGER NOT NULL DEFAULT 1,
            installed_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_skills_name ON installed_skills(name);
        CREATE INDEX IF NOT EXISTS idx_skills_enabled ON installed_skills(enabled);
        CREATE INDEX IF NOT EXISTS idx_skills_source ON installed_skills(source_type);

        PRAGMA user_version = 3;
        ")
        .execute(&mut *conn)
        .await?;

    tracing::info!("migrated to schema v3");
    Ok(())
}

async fn migrate_v4(conn: &mut AnyConnection) -> Result<()> {
    sqlx::raw_sql(
        r"
        -- Installed personas table (from marketplace)
        CREATE TABLE IF NOT EXISTS installed_personas (
            id TEXT PRIMARY KEY,
            persona_id TEXT NOT NULL UNIQUE,
            name TEXT NOT NULL,
            tagline TEXT,
            avatar TEXT,
            accent_color TEXT,
            content TEXT NOT NULL,
            source_namespace TEXT NOT NULL,
            installed_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_personas_persona_id ON installed_personas(persona_id);
        CREATE INDEX IF NOT EXISTS idx_personas_namespace ON installed_personas(source_namespace);

        PRAGMA user_version = 4;
        ",
    )
    .execute(&mut *conn)
    .await?;

    tracing::info!("migrated to schema v4");
    Ok(())
}

async fn migrate_v5(conn: &mut AnyConnection) -> Result<()> {
    // Note: sqlite-vec extension is registered globally in db::init()
    // before any connections are created

    sqlx::raw_sql(
        r"
        -- Add embedding column to memories
        ALTER TABLE memories ADD COLUMN embedding BLOB;

        -- Add source metadata columns
        ALTER TABLE memories ADD COLUMN source_session_id TEXT;
        ALTER TABLE memories ADD COLUMN source_channel TEXT;

        -- Create virtual table for vector search
        CREATE VIRTUAL TABLE IF NOT EXISTS memories_vec USING vec0(
            memory_id TEXT PRIMARY KEY,
            embedding FLOAT[1536]
        );

        PRAGMA user_version = 5;
        ",
    )
    .execute(&mut *conn)
    .await?;

    tracing::info!("migrated to schema v5 (vector search)");
    Ok(())
}

async fn migrate_v6(conn: &mut AnyConnection) -> Result<()> {
    sqlx::raw_sql(
        r"
        -- Paired users table for DM security
        CREATE TABLE IF NOT EXISTS paired_users (
            id TEXT PRIMARY KEY,
            sender_id TEXT NOT NULL,
            channel TEXT NOT NULL,
            paired_at TEXT NOT NULL,
            pairing_code TEXT,
            code_expires_at TEXT,
            UNIQUE(sender_id, channel)
        );

        CREATE INDEX IF NOT EXISTS idx_paired_users_sender ON paired_users(sender_id, channel);
        CREATE INDEX IF NOT EXISTS idx_paired_users_channel ON paired_users(channel);

        PRAGMA user_version = 6;
        ",
    )
    .execute(&mut *conn)
    .await?;

    tracing::info!("migrated to schema v6 (DM pairing)");
    Ok(())
}

async fn migrate_v7(conn: &mut AnyConnection) -> Result<()> {
    sqlx::raw_sql(
        r"
        -- Paired devices table for device identity system
        CREATE TABLE IF NOT EXISTS devices (
            id TEXT PRIMARY KEY,
            public_key BLOB NOT NULL UNIQUE,
            name TEXT NOT NULL,
            platform TEXT,
            trust_level TEXT NOT NULL DEFAULT 'paired',
            paired_at TEXT NOT NULL,
            last_seen TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_devices_public_key ON devices(public_key);
        CREATE INDEX IF NOT EXISTS idx_devices_last_seen ON devices(last_seen);

        PRAGMA user_version = 7;
        ",
    )
    .execute(&mut *conn)
    .await?;

    tracing::info!("migrated to schema v7 (device identity)");
    Ok(())
}

async fn migrate_v8(conn: &mut AnyConnection) -> Result<()> {
    sqlx::raw_sql(
        r"
        -- Add thread_id to messages for conversation threading
        ALTER TABLE messages ADD COLUMN thread_id TEXT;

        -- Index for efficient thread queries
        CREATE INDEX IF NOT EXISTS idx_messages_thread ON messages(session_id, thread_id);

        PRAGMA user_version = 8;
        ",
    )
    .execute(&mut *conn)
    .await?;

    tracing::info!("migrated to schema v8 (message threading)");
    Ok(())
}

async fn migrate_v9(conn: &mut AnyConnection) -> Result<()> {
    sqlx::raw_sql(
        r"
        -- Add sync metadata columns to memories
        ALTER TABLE memories ADD COLUMN content_hash TEXT;
        ALTER TABLE memories ADD COLUMN origin_device_id TEXT;
        ALTER TABLE memories ADD COLUMN updated_at TEXT NOT NULL DEFAULT (datetime('now'));
        ALTER TABLE memories ADD COLUMN deleted_at TEXT;
        ALTER TABLE memories ADD COLUMN synced_at TEXT;
        ALTER TABLE memories ADD COLUMN cloud_id TEXT;

        CREATE INDEX IF NOT EXISTS idx_memories_synced ON memories(synced_at);
        CREATE INDEX IF NOT EXISTS idx_memories_updated ON memories(updated_at);

        PRAGMA user_version = 9;
        ",
    )
    .execute(&mut *conn)
    .await?;

    // Backfill content_hash for existing memories
    backfill_content_hashes(conn).await?;

    tracing::info!("migrated to schema v9 (memory sync)");
    Ok(())
}

/// Backfill `content_hash` for existing memories that lack one
async fn backfill_content_hashes(conn: &mut AnyConnection) -> Result<()> {
    use sha2::{Digest, Sha256};

    let rows = sqlx::query("SELECT id, content FROM memories WHERE content_hash IS NULL")
        .fetch_all(&mut *conn)
        .await?;

    let count = rows.len();
    for row in rows {
        let id: String = row.try_get(0)?;
        let content: String = row.try_get(1)?;
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        let hash = hex::encode(hasher.finalize());

        sqlx::query("UPDATE memories SET content_hash = $1 WHERE id = $2")
            .bind(hash)
            .bind(id)
            .execute(&mut *conn)
            .await?;
    }

    if count > 0 {
        tracing::info!(count, "backfilled content hashes for existing memories");
    }

    Ok(())
}

async fn migrate_v10(conn: &mut AnyConnection) -> Result<()> {
    sqlx::raw_sql(
        r"
        -- Installed knowledge packs table
        CREATE TABLE IF NOT EXISTS installed_knowledge_packs (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            version TEXT NOT NULL,
            source_namespace TEXT NOT NULL,
            description TEXT,
            tags TEXT NOT NULL DEFAULT '[]',
            chunk_count INTEGER NOT NULL DEFAULT 0,
            has_embeddings INTEGER NOT NULL DEFAULT 0,
            content TEXT NOT NULL,
            installed_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(name, source_namespace)
        );

        -- Vector table for knowledge chunk embeddings
        CREATE VIRTUAL TABLE IF NOT EXISTS knowledge_vec USING vec0(
            chunk_id TEXT PRIMARY KEY,
            embedding FLOAT[1536]
        );

        PRAGMA user_version = 10;
        ",
    )
    .execute(&mut *conn)
    .await?;

    tracing::info!("migrated to schema v10 (knowledge packs)");
    Ok(())
}

async fn migrate_v11(conn: &mut AnyConnection) -> Result<()> {
    sqlx::raw_sql(
        r"
        -- Gateway-local provider keys for self-hosted deployments
        CREATE TABLE IF NOT EXISTS local_provider_keys (
            provider TEXT PRIMARY KEY,
            api_key TEXT NOT NULL,
            model_preference TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        PRAGMA user_version = 11;
        ",
    )
    .execute(&mut *conn)
    .await?;

    tracing::info!("migrated to schema v11 (local provider keys)");
    Ok(())
}

async fn migrate_v12(conn: &mut AnyConnection) -> Result<()> {
    sqlx::raw_sql(
        r"
        -- Add priority column to installed_skills for prompt hierarchy
        ALTER TABLE installed_skills ADD COLUMN priority TEXT NOT NULL DEFAULT 'standard';

        CREATE INDEX IF NOT EXISTS idx_skills_priority ON installed_skills(priority);

        PRAGMA user_version = 12;
        ",
    )
    .execute(&mut *conn)
    .await?;

    tracing::info!("migrated to schema v12 (skill priority)");
    Ok(())
}

async fn migrate_v13(conn: &mut AnyConnection) -> Result<()> {
    sqlx::raw_sql(
        r"
        -- Extended skill metadata and per-user scoping
        ALTER TABLE installed_skills ADD COLUMN always_include INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE installed_skills ADD COLUMN user_invocable INTEGER NOT NULL DEFAULT 1;
        ALTER TABLE installed_skills ADD COLUMN disable_model_invocation INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE installed_skills ADD COLUMN emoji TEXT;
        ALTER TABLE installed_skills ADD COLUMN requires_env TEXT NOT NULL DEFAULT '[]';
        ALTER TABLE installed_skills ADD COLUMN command_name TEXT;
        ALTER TABLE installed_skills ADD COLUMN user_id TEXT;

        CREATE INDEX IF NOT EXISTS idx_skills_command ON installed_skills(command_name);
        CREATE INDEX IF NOT EXISTS idx_skills_user ON installed_skills(user_id);

        PRAGMA user_version = 13;
        ",
    )
    .execute(&mut *conn)
    .await?;

    tracing::info!("migrated to schema v13 (extended skill metadata + per-user scoping)");
    Ok(())
}

async fn migrate_v14(conn: &mut AnyConnection) -> Result<()> {
    sqlx::raw_sql(
        r"
        -- OS, binary, dispatch, and env fields for skills
        ALTER TABLE installed_skills ADD COLUMN os TEXT NOT NULL DEFAULT '[]';
        ALTER TABLE installed_skills ADD COLUMN requires_bins TEXT NOT NULL DEFAULT '[]';
        ALTER TABLE installed_skills ADD COLUMN requires_any_bins TEXT NOT NULL DEFAULT '[]';
        ALTER TABLE installed_skills ADD COLUMN primary_env TEXT;
        ALTER TABLE installed_skills ADD COLUMN command_dispatch_tool TEXT;
        ALTER TABLE installed_skills ADD COLUMN api_key TEXT;
        ALTER TABLE installed_skills ADD COLUMN skill_env TEXT NOT NULL DEFAULT '{}';

        PRAGMA user_version = 14;
        ",
    )
    .execute(&mut *conn)
    .await?;

    tracing::info!("migrated to schema v14 (OS, bins, dispatch, per-skill env)");
    Ok(())
}

async fn migrate_v15(conn: &mut AnyConnection) -> Result<()> {
    // Recreate table to update CHECK constraint (add 'plugin' source_type)
    // and add install_specs column
    sqlx::raw_sql(r"
        -- Recreate installed_skills with updated source_type CHECK and new column
        CREATE TABLE IF NOT EXISTS installed_skills_new (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            description TEXT NOT NULL,
            version TEXT,
            author TEXT,
            tags TEXT NOT NULL DEFAULT '[]',
            permissions TEXT NOT NULL DEFAULT '[]',
            content TEXT NOT NULL,
            source_type TEXT NOT NULL CHECK(source_type IN ('local', 'manifold', 'bundled', 'plugin')),
            source_namespace TEXT,
            source_repository TEXT,
            enabled INTEGER NOT NULL DEFAULT 1,
            installed_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            priority TEXT NOT NULL DEFAULT 'standard',
            always_include INTEGER NOT NULL DEFAULT 0,
            user_invocable INTEGER NOT NULL DEFAULT 1,
            disable_model_invocation INTEGER NOT NULL DEFAULT 0,
            emoji TEXT,
            requires_env TEXT NOT NULL DEFAULT '[]',
            command_name TEXT,
            user_id TEXT,
            os TEXT NOT NULL DEFAULT '[]',
            requires_bins TEXT NOT NULL DEFAULT '[]',
            requires_any_bins TEXT NOT NULL DEFAULT '[]',
            primary_env TEXT,
            command_dispatch_tool TEXT,
            api_key TEXT,
            skill_env TEXT NOT NULL DEFAULT '{}',
            install_specs TEXT NOT NULL DEFAULT '[]'
        );

        INSERT OR IGNORE INTO installed_skills_new
            SELECT id, name, description, version, author, tags, permissions,
                   content, source_type, source_namespace, source_repository,
                   enabled, installed_at, COALESCE(updated_at, datetime('now')),
                   COALESCE(priority, 'standard'),
                   COALESCE(always_include, 0), COALESCE(user_invocable, 1),
                   COALESCE(disable_model_invocation, 0), emoji,
                   COALESCE(requires_env, '[]'), command_name, user_id,
                   COALESCE(os, '[]'), COALESCE(requires_bins, '[]'),
                   COALESCE(requires_any_bins, '[]'), primary_env,
                   command_dispatch_tool, api_key,
                   COALESCE(skill_env, '{}'), '[]'
            FROM installed_skills;

        DROP TABLE installed_skills;
        ALTER TABLE installed_skills_new RENAME TO installed_skills;

        -- Re-create indices
        CREATE INDEX IF NOT EXISTS idx_skills_name ON installed_skills(name);
        CREATE INDEX IF NOT EXISTS idx_skills_enabled ON installed_skills(enabled);
        CREATE INDEX IF NOT EXISTS idx_skills_source ON installed_skills(source_type);
        CREATE INDEX IF NOT EXISTS idx_skills_priority ON installed_skills(priority);
        CREATE INDEX IF NOT EXISTS idx_skills_command ON installed_skills(command_name);
        CREATE INDEX IF NOT EXISTS idx_skills_user ON installed_skills(user_id);

        PRAGMA user_version = 15;
        ")
        .execute(&mut *conn)
        .await?;

    tracing::info!("migrated to schema v15 (skill install specs, plugin source type)");
    Ok(())
}

async fn migrate_v16(conn: &mut AnyConnection) -> Result<()> {
    sqlx::raw_sql(
        r"
        -- Config-based eligibility paths
        ALTER TABLE installed_skills ADD COLUMN requires_config TEXT NOT NULL DEFAULT '[]';

        PRAGMA user_version = 16;
        ",
    )
    .execute(&mut *conn)
    .await?;

    tracing::info!("migrated to schema v16 (config-based eligibility)");
    Ok(())
}

async fn migrate_v17(conn: &mut AnyConnection) -> Result<()> {
    sqlx::raw_sql(
        r"
        -- Per-group Telegram configuration overrides
        CREATE TABLE IF NOT EXISTS telegram_group_config (
            chat_id TEXT PRIMARY KEY,
            chat_title TEXT,
            require_mention INTEGER,
            reaction_level TEXT,
            ack_reaction TEXT,
            done_reaction TEXT,
            enabled INTEGER NOT NULL DEFAULT 1,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        PRAGMA user_version = 17;
        ",
    )
    .execute(&mut *conn)
    .await?;

    tracing::info!("migrated to schema v17 (per-group Telegram config)");
    Ok(())
}

async fn migrate_v18(conn: &mut AnyConnection) -> Result<()> {
    sqlx::raw_sql(
        r"
        -- Skill file location for compact prompt mode
        ALTER TABLE installed_skills ADD COLUMN location TEXT;

        PRAGMA user_version = 18;
        ",
    )
    .execute(&mut *conn)
    .await?;

    tracing::info!("migrated to schema v18 (skill location)");
    Ok(())
}

async fn migrate_v19(conn: &mut AnyConnection) -> Result<()> {
    sqlx::raw_sql(
        r"
        -- Add agent_id to sessions for multi-agent support
        ALTER TABLE sessions ADD COLUMN agent_id TEXT NOT NULL DEFAULT 'default';

        -- Update unique index to include agent_id
        CREATE INDEX IF NOT EXISTS idx_sessions_agent ON sessions(agent_id);

        PRAGMA user_version = 19;
        ",
    )
    .execute(&mut *conn)
    .await?;

    tracing::info!("migrated to schema v19 (multi-agent sessions)");
    Ok(())
}

async fn migrate_v20(conn: &mut AnyConnection) -> Result<()> {
    sqlx::raw_sql(
        r"
        -- Local usage tracking table
        CREATE TABLE IF NOT EXISTS usage_records (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            agent_id TEXT NOT NULL DEFAULT 'default',
            model TEXT NOT NULL,
            provider TEXT NOT NULL,
            input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            estimated_cost_usd REAL NOT NULL DEFAULT 0.0,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_usage_session ON usage_records(session_id);
        CREATE INDEX IF NOT EXISTS idx_usage_created ON usage_records(created_at);
        CREATE INDEX IF NOT EXISTS idx_usage_model ON usage_records(model);

        PRAGMA user_version = 20;
        ",
    )
    .execute(&mut *conn)
    .await?;

    tracing::info!("migrated to schema v20 (usage tracking)");
    Ok(())
}

async fn migrate_v21(conn: &mut AnyConnection) -> Result<()> {
    sqlx::raw_sql(
        r"
        CREATE TABLE IF NOT EXISTS harness_sessions (
            id TEXT PRIMARY KEY,
            agent_id TEXT NOT NULL,
            channel TEXT NOT NULL,
            channel_id TEXT NOT NULL,
            harness_session_id TEXT NOT NULL,
            adapter TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(agent_id, channel, channel_id)
        );

        PRAGMA user_version = 21;
        ",
    )
    .execute(&mut *conn)
    .await?;

    tracing::info!("migrated to schema v21 (harness sessions)");
    Ok(())
}

async fn migrate_v22(conn: &mut AnyConnection) -> Result<()> {
    sqlx::raw_sql(
        r"
        -- The personal workspace each Gatekeeper user's usage bills, so work
        -- without a token (scheduled check-ins, workflows) still bills it
        CREATE TABLE IF NOT EXISTS billing_workspaces (
            user_id TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL,
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        PRAGMA user_version = 22;
        ",
    )
    .execute(&mut *conn)
    .await?;

    tracing::info!("migrated to schema v22 (billing workspaces)");
    Ok(())
}

/// Postgres versions are tracked in a table rather than a pragma
async fn init_postgres(conn: &mut AnyConnection) -> Result<()> {
    sqlx::raw_sql(
        r"
        CREATE TABLE IF NOT EXISTS schema_migrations (
            version BIGINT PRIMARY KEY,
            applied_at TEXT NOT NULL
        );
        ",
    )
    .execute(&mut *conn)
    .await?;

    let version: i64 = sqlx::query("SELECT COALESCE(MAX(version), 0) FROM schema_migrations")
        .fetch_one(&mut *conn)
        .await?
        .try_get(0)?;

    if version < 22 {
        postgres_baseline(conn).await?;
    }

    Ok(())
}

/// Postgres schema at version 22, mirroring the `SQLite` schema after its
/// migrations v1 to v22: 64-bit integers, `TEXT` timestamps in the same format
/// (`YYYY-MM-DD HH:MM:SS` UTC), and pgvector tables in place of sqlite-vec
async fn postgres_baseline(conn: &mut AnyConnection) -> Result<()> {
    sqlx::raw_sql(include_str!("schema_postgres.sql"))
        .execute(&mut *conn)
        .await?;

    sqlx::query("INSERT INTO schema_migrations (version, applied_at) VALUES ($1, $2)")
        .bind(22_i64)
        .bind(super::now())
        .execute(&mut *conn)
        .await?;

    tracing::info!("created postgres schema v22 (baseline)");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn table_count(db: &DbPool, name: &str) -> i64 {
        let sql = match db.backend() {
            Backend::Sqlite => {
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = $1"
            }
            Backend::Postgres => {
                "SELECT COUNT(*) FROM information_schema.tables \
                 WHERE table_schema = current_schema() AND table_name = $1"
            }
        };
        sqlx::query_scalar(sql)
            .bind(name.to_string())
            .fetch_one(db.pool())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn test_schema_init() {
        let db = crate::db::init_memory().await.unwrap();
        assert_eq!(table_count(&db, "users").await, 1);
        assert_eq!(table_count(&db, "memories_vec").await, 1);
    }

    #[tokio::test]
    async fn test_schema_idempotent() {
        let db = crate::db::init_memory().await.unwrap();
        init(&db).await.unwrap();
    }

    #[tokio::test]
    async fn test_vector_extension_loaded() {
        let db = crate::db::init_memory().await.unwrap();
        let sql = match db.backend() {
            Backend::Sqlite => "SELECT vec_version()",
            Backend::Postgres => "SELECT extversion FROM pg_extension WHERE extname = 'vector'",
        };
        let version: String = sqlx::query_scalar(sql).fetch_one(db.pool()).await.unwrap();
        assert!(!version.is_empty());
    }
}
