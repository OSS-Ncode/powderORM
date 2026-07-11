//! Live PostgreSQL migration/validation test — runs only when `POWDER_PG_URL`
//! points at a reachable server. Without the env var it passes as a skip, so
//! machines without Postgres stay green.

use powder_cli::db::{self};
use powder_cli::schema::Schema;

#[test]
fn postgres_migrate_seed_validate_roundtrip() {
    let Ok(url) = std::env::var("POWDER_PG_URL") else {
        eprintln!("POWDER_PG_URL not set; skipping live postgres migration test");
        return;
    };

    let schema = Schema::parse(
        r#"{"tables":{
            "pg_mig_users":{"columns":{
                "id":{"type":"int","primaryKey":true},
                "name":{"type":"text"},
                "score":{"type":"float","nullable":true},
                "active":{"type":"bool"}
            }},
            "pg_mig_posts":{"columns":{
                "id":{"type":"int","primaryKey":true},
                "user_id":{"type":"int","references":{"table":"pg_mig_users","column":"id"}},
                "title":{"type":"text"}
            }}
        }}"#,
    )
    .unwrap();

    let mut conn = db::open(&url).expect("connect");
    // Clean slate (order matters for the FK).
    conn.execute_batch("DROP TABLE IF EXISTS pg_mig_posts; DROP TABLE IF EXISTS pg_mig_users")
        .unwrap();

    let applied = db::migrate(&mut conn, &schema).expect("migrate");
    assert_eq!(applied.len(), 2, "{applied:?}");
    assert!(applied[0].contains("BIGINT"), "postgres types: {applied:?}");

    // Idempotent + in sync.
    assert!(db::migrate(&mut conn, &schema).unwrap().is_empty());
    let problems = db::validate(&mut conn, &schema).expect("validate");
    assert!(problems.is_empty(), "{problems:?}");

    // Seed through the JSON path ($n placeholders under the hood).
    let n = db::seed(
        &mut conn,
        "seed.json",
        r#"{"pg_mig_users": [{"id": 1, "name": "alice", "score": 9.5, "active": true}],
            "pg_mig_posts": [{"id": 1, "user_id": 1, "title": "hello"}]}"#,
    )
    .expect("seed");
    assert_eq!(n, 2);

    // Additive migration: add a column to the schema, re-migrate.
    let extended = Schema::parse(
        r#"{"tables":{
            "pg_mig_users":{"columns":{
                "id":{"type":"int","primaryKey":true},
                "name":{"type":"text"},
                "score":{"type":"float","nullable":true},
                "active":{"type":"bool"},
                "bio":{"type":"text","nullable":true}
            }},
            "pg_mig_posts":{"columns":{
                "id":{"type":"int","primaryKey":true},
                "user_id":{"type":"int","references":{"table":"pg_mig_users","column":"id"}},
                "title":{"type":"text"}
            }}
        }}"#,
    )
    .unwrap();
    let applied = db::migrate(&mut conn, &extended).unwrap();
    assert_eq!(applied.len(), 1);
    assert!(applied[0].contains("ADD COLUMN bio"), "{applied:?}");
    assert!(db::validate(&mut conn, &extended).unwrap().is_empty());

    // Drift is detected: the original schema now sees an extra column.
    let problems = db::validate(&mut conn, &schema).unwrap();
    assert!(problems.iter().any(|p| p.contains("extra column `bio`")), "{problems:?}");

    // --rebuild is explicitly rejected on postgres.
    assert!(db::migrate_rebuild(&mut conn, &schema).unwrap_err().contains("SQLite-only"));

    conn.execute_batch("DROP TABLE pg_mig_posts; DROP TABLE pg_mig_users").unwrap();
}
