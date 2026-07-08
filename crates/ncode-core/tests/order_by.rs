//! The in-engine sort pull-up must be indistinguishable from SQLite's own
//! ORDER BY. Every case here compares the fast path (or its fallback) against
//! expected SQLite semantics on deliberately shuffled data.

use ncode_core::{Client, Value};

async fn seed(client: &Client, rows: &[(i64, &str, f64)]) {
    client
        .execute(
            "CREATE TABLE t (id INTEGER, name TEXT, score REAL)",
            vec![],
        )
        .await
        .unwrap();
    for (id, name, score) in rows {
        client
            .execute(
                "INSERT INTO t VALUES (?, ?, ?)",
                vec![Value::Int(*id), Value::Text(name.to_string()), Value::Float(*score)],
            )
            .await
            .unwrap();
    }
}

const SHUFFLED: &[(i64, &str, f64)] = &[
    (3, "carol", 1.5),
    (1, "alice", -2.0),
    (4, "dave", 0.0),
    (2, "bob", 9.75),
];

#[tokio::test]
async fn order_by_int_asc_on_unsorted_data() {
    let client = Client::connect(":memory:").await.unwrap();
    seed(&client, SHUFFLED).await;
    let batch = client
        .query("SELECT id, name, score FROM t ORDER BY id ASC", vec![])
        .await
        .unwrap();
    let ids: Vec<_> = (0..4).map(|r| batch.column("id").unwrap().i64(r).unwrap()).collect();
    assert_eq!(ids, [1, 2, 3, 4]);
    // Payload columns must be permuted in lockstep with the key.
    let names: Vec<_> = (0..4)
        .map(|r| batch.column("name").unwrap().str(r).unwrap().to_string())
        .collect();
    assert_eq!(names, ["alice", "bob", "carol", "dave"]);
    let scores: Vec<_> = (0..4).map(|r| batch.column("score").unwrap().f64(r).unwrap()).collect();
    assert_eq!(scores, [-2.0, 9.75, 1.5, 0.0]);
}

#[tokio::test]
async fn order_by_int_desc() {
    let client = Client::connect(":memory:").await.unwrap();
    seed(&client, SHUFFLED).await;
    let batch = client
        .query("SELECT id, name FROM t ORDER BY id DESC", vec![])
        .await
        .unwrap();
    let ids: Vec<_> = (0..4).map(|r| batch.column("id").unwrap().i64(r).unwrap()).collect();
    assert_eq!(ids, [4, 3, 2, 1]);
}

#[tokio::test]
async fn order_by_text_matches_binary_collation() {
    let client = Client::connect(":memory:").await.unwrap();
    seed(&client, SHUFFLED).await;
    let batch = client
        .query("SELECT name FROM t ORDER BY name DESC", vec![])
        .await
        .unwrap();
    let names: Vec<_> = (0..4)
        .map(|r| batch.column("name").unwrap().str(r).unwrap().to_string())
        .collect();
    assert_eq!(names, ["dave", "carol", "bob", "alice"]);
}

#[tokio::test]
async fn order_by_float() {
    let client = Client::connect(":memory:").await.unwrap();
    seed(&client, SHUFFLED).await;
    let batch = client
        .query("SELECT score FROM t ORDER BY score ASC", vec![])
        .await
        .unwrap();
    let scores: Vec<_> = (0..4).map(|r| batch.column("score").unwrap().f64(r).unwrap()).collect();
    assert_eq!(scores, [-2.0, 0.0, 1.5, 9.75]);
}

#[tokio::test]
async fn order_by_nullable_key_falls_back_with_nulls_first() {
    let client = Client::connect(":memory:").await.unwrap();
    client
        .execute("CREATE TABLE t (id INTEGER)", vec![])
        .await
        .unwrap();
    client
        .execute(
            "INSERT INTO t VALUES (2), (NULL), (1)",
            vec![],
        )
        .await
        .unwrap();
    let batch = client
        .query("SELECT id FROM t ORDER BY id ASC", vec![])
        .await
        .unwrap();
    // SQLite semantics: NULL sorts first on ASC.
    let id = batch.column("id").unwrap();
    assert_eq!(id.i64(0), None);
    assert_eq!(id.i64(1), Some(1));
    assert_eq!(id.i64(2), Some(2));
}

#[tokio::test]
async fn order_by_mixed_type_key_falls_back_to_sqlite_class_order() {
    let client = Client::connect(":memory:").await.unwrap();
    client.execute("CREATE TABLE t (v)", vec![]).await.unwrap();
    // SQLite orders storage classes: numeric < text. A naive stringified sort
    // would give "10" < "9" < "apple"; class order gives 9 < 10 < 'apple'.
    client
        .execute("INSERT INTO t VALUES ('apple'), (10), (9)", vec![])
        .await
        .unwrap();
    let batch = client
        .query("SELECT v FROM t ORDER BY v ASC", vec![])
        .await
        .unwrap();
    let v = batch.column("v").unwrap();
    assert_eq!(v.str(0), Some("9"));
    assert_eq!(v.str(1), Some("10"));
    assert_eq!(v.str(2), Some("apple"));
}

#[tokio::test]
async fn order_by_column_not_in_output_falls_back() {
    let client = Client::connect(":memory:").await.unwrap();
    seed(&client, SHUFFLED).await;
    let batch = client
        .query("SELECT name FROM t ORDER BY id ASC", vec![])
        .await
        .unwrap();
    let names: Vec<_> = (0..4)
        .map(|r| batch.column("name").unwrap().str(r).unwrap().to_string())
        .collect();
    assert_eq!(names, ["alice", "bob", "carol", "dave"]);
}

#[tokio::test]
async fn order_by_with_limit_stays_on_sqlite_path() {
    let client = Client::connect(":memory:").await.unwrap();
    seed(&client, SHUFFLED).await;
    let batch = client
        .query("SELECT id FROM t ORDER BY id DESC LIMIT 2", vec![])
        .await
        .unwrap();
    assert_eq!(batch.num_rows, 2);
    assert_eq!(batch.column("id").unwrap().i64(0), Some(4));
    assert_eq!(batch.column("id").unwrap().i64(1), Some(3));
}

#[tokio::test]
async fn order_by_with_where_and_params() {
    let client = Client::connect(":memory:").await.unwrap();
    seed(&client, SHUFFLED).await;
    let batch = client
        .query(
            "SELECT id, name FROM t WHERE id >= ? ORDER BY id DESC",
            vec![Value::Int(2)],
        )
        .await
        .unwrap();
    let ids: Vec<_> = (0..3).map(|r| batch.column("id").unwrap().i64(r).unwrap()).collect();
    assert_eq!(ids, [4, 3, 2]);
}

#[tokio::test]
async fn order_by_collate_stays_on_sqlite_path() {
    let client = Client::connect(":memory:").await.unwrap();
    client.execute("CREATE TABLE t (name TEXT)", vec![]).await.unwrap();
    client
        .execute("INSERT INTO t VALUES ('b'), ('A'), ('a')", vec![])
        .await
        .unwrap();
    let batch = client
        .query("SELECT name FROM t ORDER BY name COLLATE NOCASE ASC", vec![])
        .await
        .unwrap();
    // NOCASE: 'A'/'a' tie before 'b' — binary sort would put 'A' first, 'a' last.
    let first = batch.column("name").unwrap().str(0).unwrap().to_lowercase();
    let last = batch.column("name").unwrap().str(2).unwrap().to_lowercase();
    assert_eq!(first, "a");
    assert_eq!(last, "b");
}
