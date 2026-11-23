-- Postgres baseline: the SQLite schema after migrations v1 to v22
--
-- Kept column for column with SQLite (tests/schema_parity.rs enforces it):
-- 64-bit integers, booleans as integers, timestamps as TEXT in the format
-- `YYYY-MM-DD HH:MM:SS` (UTC), and pgvector tables in place of sqlite-vec

CREATE EXTENSION IF NOT EXISTS vector;

CREATE TABLE IF NOT EXISTS users (
    id TEXT PRIMARY KEY,
    life_json_path TEXT,
    created_at TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS'),
    updated_at TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS')
);

CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id),
    channel TEXT NOT NULL,
    channel_id TEXT NOT NULL,
    persona_id TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS'),
    updated_at TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS'),
    agent_id TEXT NOT NULL DEFAULT 'default'
);
CREATE INDEX IF NOT EXISTS idx_sessions_user ON sessions(user_id);
CREATE INDEX IF NOT EXISTS idx_sessions_channel ON sessions(channel, channel_id);
CREATE INDEX IF NOT EXISTS idx_sessions_agent ON sessions(agent_id);

CREATE TABLE IF NOT EXISTS messages (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    role TEXT NOT NULL CHECK (role IN ('user', 'assistant', 'system')),
    content TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS'),
    thread_id TEXT
);
CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id);
CREATE INDEX IF NOT EXISTS idx_messages_thread ON messages(session_id, thread_id);

CREATE TABLE IF NOT EXISTS user_context (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id),
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    source TEXT NOT NULL DEFAULT 'learned',
    created_at TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS'),
    updated_at TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS'),
    UNIQUE (user_id, key)
);
CREATE INDEX IF NOT EXISTS idx_user_context_user ON user_context(user_id);

CREATE TABLE IF NOT EXISTS memories (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id),
    category TEXT NOT NULL CHECK (category IN ('preference', 'fact', 'correction', 'general')),
    content TEXT NOT NULL,
    tags TEXT NOT NULL DEFAULT '[]',
    pinned BIGINT NOT NULL DEFAULT 0,
    access_count BIGINT NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS'),
    accessed_at TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS'),
    embedding BYTEA,
    source_session_id TEXT,
    source_channel TEXT,
    content_hash TEXT,
    origin_device_id TEXT,
    updated_at TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS'),
    deleted_at TEXT,
    synced_at TEXT,
    cloud_id TEXT
);
CREATE INDEX IF NOT EXISTS idx_memories_user ON memories(user_id);
CREATE INDEX IF NOT EXISTS idx_memories_category ON memories(category);
CREATE INDEX IF NOT EXISTS idx_memories_pinned ON memories(pinned);
CREATE INDEX IF NOT EXISTS idx_memories_synced ON memories(synced_at);
CREATE INDEX IF NOT EXISTS idx_memories_updated ON memories(updated_at);

-- Vector search (sqlite-vec's vec0 tables on SQLite); L2 distance in both
CREATE TABLE IF NOT EXISTS memories_vec (
    memory_id TEXT PRIMARY KEY,
    embedding vector(1536) NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_memories_vec_embedding
    ON memories_vec USING hnsw (embedding vector_l2_ops);

CREATE TABLE IF NOT EXISTS installed_skills (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT NOT NULL,
    version TEXT,
    author TEXT,
    tags TEXT NOT NULL DEFAULT '[]',
    permissions TEXT NOT NULL DEFAULT '[]',
    content TEXT NOT NULL,
    source_type TEXT NOT NULL CHECK (source_type IN ('local', 'manifold', 'bundled', 'plugin')),
    source_namespace TEXT,
    source_repository TEXT,
    enabled BIGINT NOT NULL DEFAULT 1,
    installed_at TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS'),
    updated_at TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS'),
    priority TEXT NOT NULL DEFAULT 'standard',
    always_include BIGINT NOT NULL DEFAULT 0,
    user_invocable BIGINT NOT NULL DEFAULT 1,
    disable_model_invocation BIGINT NOT NULL DEFAULT 0,
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
    install_specs TEXT NOT NULL DEFAULT '[]',
    requires_config TEXT NOT NULL DEFAULT '[]',
    location TEXT
);
CREATE INDEX IF NOT EXISTS idx_skills_command ON installed_skills(command_name);
CREATE INDEX IF NOT EXISTS idx_skills_enabled ON installed_skills(enabled);
CREATE INDEX IF NOT EXISTS idx_skills_name ON installed_skills(name);
CREATE INDEX IF NOT EXISTS idx_skills_priority ON installed_skills(priority);
CREATE INDEX IF NOT EXISTS idx_skills_source ON installed_skills(source_type);
CREATE INDEX IF NOT EXISTS idx_skills_user ON installed_skills(user_id);

CREATE TABLE IF NOT EXISTS installed_personas (
    id TEXT PRIMARY KEY,
    persona_id TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    tagline TEXT,
    avatar TEXT,
    accent_color TEXT,
    content TEXT NOT NULL,
    source_namespace TEXT NOT NULL,
    installed_at TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS'),
    updated_at TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS')
);
CREATE INDEX IF NOT EXISTS idx_personas_namespace ON installed_personas(source_namespace);
CREATE INDEX IF NOT EXISTS idx_personas_persona_id ON installed_personas(persona_id);

CREATE TABLE IF NOT EXISTS installed_knowledge_packs (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    version TEXT NOT NULL,
    source_namespace TEXT NOT NULL,
    description TEXT,
    tags TEXT NOT NULL DEFAULT '[]',
    chunk_count BIGINT NOT NULL DEFAULT 0,
    has_embeddings BIGINT NOT NULL DEFAULT 0,
    content TEXT NOT NULL,
    installed_at TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS'),
    updated_at TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS'),
    UNIQUE (name, source_namespace)
);

CREATE TABLE IF NOT EXISTS knowledge_vec (
    chunk_id TEXT PRIMARY KEY,
    embedding vector(1536) NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_knowledge_vec_embedding
    ON knowledge_vec USING hnsw (embedding vector_l2_ops);

CREATE TABLE IF NOT EXISTS devices (
    id TEXT PRIMARY KEY,
    -- Stored as text (base64), as the code writes it; SQLite declares BLOB
    public_key TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    platform TEXT,
    trust_level TEXT NOT NULL DEFAULT 'paired',
    paired_at TEXT NOT NULL,
    last_seen TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_devices_last_seen ON devices(last_seen);
CREATE INDEX IF NOT EXISTS idx_devices_public_key ON devices(public_key);

CREATE TABLE IF NOT EXISTS paired_users (
    id TEXT PRIMARY KEY,
    sender_id TEXT NOT NULL,
    channel TEXT NOT NULL,
    paired_at TEXT NOT NULL,
    pairing_code TEXT,
    code_expires_at TEXT,
    UNIQUE (sender_id, channel)
);
CREATE INDEX IF NOT EXISTS idx_paired_users_channel ON paired_users(channel);
CREATE INDEX IF NOT EXISTS idx_paired_users_sender ON paired_users(sender_id, channel);

CREATE TABLE IF NOT EXISTS usage_records (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    agent_id TEXT NOT NULL DEFAULT 'default',
    model TEXT NOT NULL,
    provider TEXT NOT NULL,
    input_tokens BIGINT NOT NULL DEFAULT 0,
    output_tokens BIGINT NOT NULL DEFAULT 0,
    estimated_cost_usd DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    created_at TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS')
);
CREATE INDEX IF NOT EXISTS idx_usage_created ON usage_records(created_at);
CREATE INDEX IF NOT EXISTS idx_usage_model ON usage_records(model);
CREATE INDEX IF NOT EXISTS idx_usage_session ON usage_records(session_id);

CREATE TABLE IF NOT EXISTS telegram_group_config (
    chat_id TEXT PRIMARY KEY,
    chat_title TEXT,
    require_mention BIGINT,
    reaction_level TEXT,
    ack_reaction TEXT,
    done_reaction TEXT,
    enabled BIGINT NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS'),
    updated_at TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS')
);

CREATE TABLE IF NOT EXISTS local_provider_keys (
    provider TEXT PRIMARY KEY,
    api_key TEXT NOT NULL,
    model_preference TEXT,
    created_at TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS'),
    updated_at TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS')
);

CREATE TABLE IF NOT EXISTS harness_sessions (
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL,
    channel TEXT NOT NULL,
    channel_id TEXT NOT NULL,
    harness_session_id TEXT NOT NULL,
    adapter TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS'),
    updated_at TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS'),
    UNIQUE (agent_id, channel, channel_id)
);

CREATE TABLE IF NOT EXISTS billing_workspaces (
    user_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT to_char(now() AT TIME ZONE 'utc', 'YYYY-MM-DD HH24:MI:SS')
);
