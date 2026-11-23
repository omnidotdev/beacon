//! Schema parity between the `SQLite` and Postgres backends
//!
//! The same repository SQL runs on both, so every table must have the same
//! columns on each. A migration added for one backend but not the other fails
//! here. Runs when `BEACON_TEST_DATABASE_URL` names a Postgres database (CI
//! provides one); otherwise it only checks the `SQLite` side is readable

use std::collections::{BTreeMap, BTreeSet};

use beacon_gateway::db::{self, Backend};
use sqlx::Row;

/// Table name to column names
type Schema = BTreeMap<String, BTreeSet<String>>;

/// Bookkeeping tables that legitimately exist on only one backend
const BACKEND_ONLY: &[&str] = &["schema_migrations"];

async fn sqlite_schema() -> Schema {
    // A SQLite file regardless of the test database variable
    let db =
        db::init(std::env::temp_dir().join(format!("beacon-parity-{}.db", uuid::Uuid::new_v4())))
            .await
            .unwrap();
    assert_eq!(db.backend(), Backend::Sqlite);

    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
    )
    .fetch_all(db.pool())
    .await
    .unwrap();

    let mut schema = Schema::new();
    for table in tables {
        // sqlite-vec keeps its index in shadow tables named after the vec0 table
        if table.contains("_vec_") {
            continue;
        }
        let columns = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT name FROM pragma_table_info('{table}')"
        )))
        .fetch_all(db.pool())
        .await
        .unwrap()
        .iter()
        .map(|row| row.get::<String, _>(0))
        .collect();
        schema.insert(table, columns);
    }
    schema
}

/// A fresh isolated Postgres schema (the suite's `init_memory` reads the same
/// test database variable)
async fn postgres_schema() -> Schema {
    let db = db::init_memory().await.unwrap();
    assert_eq!(db.backend(), Backend::Postgres);

    let rows = sqlx::query(
        "SELECT CAST(table_name AS TEXT), CAST(column_name AS TEXT) FROM information_schema.columns
         WHERE table_schema = current_schema()",
    )
    .fetch_all(db.pool())
    .await
    .unwrap();

    let mut schema = Schema::new();
    for row in rows {
        schema
            .entry(row.get::<String, _>(0))
            .or_default()
            .insert(row.get::<String, _>(1));
    }
    schema
}

fn without_backend_only(mut schema: Schema) -> Schema {
    for table in BACKEND_ONLY {
        schema.remove(*table);
    }
    schema
}

#[tokio::test]
async fn sqlite_and_postgres_schemas_match() {
    let sqlite = without_backend_only(sqlite_schema().await);
    assert!(sqlite.contains_key("memories"), "sqlite schema: {sqlite:?}");

    let Ok(url) = std::env::var(db::TEST_DATABASE_URL_ENV) else {
        eprintln!(
            "{} not set, skipping the Postgres side",
            db::TEST_DATABASE_URL_ENV
        );
        return;
    };
    if url.is_empty() {
        return;
    }

    let postgres = without_backend_only(postgres_schema().await);

    let sqlite_tables: BTreeSet<_> = sqlite.keys().collect();
    let postgres_tables: BTreeSet<_> = postgres.keys().collect();
    assert_eq!(
        sqlite_tables, postgres_tables,
        "tables differ between SQLite and Postgres"
    );

    for (table, columns) in &sqlite {
        assert_eq!(
            Some(columns),
            postgres.get(table),
            "columns of `{table}` differ between SQLite and Postgres"
        );
    }
}
