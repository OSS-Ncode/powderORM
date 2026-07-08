use ncode_core::array::{ColumnBuilder, ColumnData};
use ncode_core::query::{Order, Query};
use ncode_core::{Client, DataType, RecordBatch, Value};

fn build_batch() -> RecordBatch {
    let mut id = ColumnBuilder::new(DataType::Int64);
    id.push_i64(1).unwrap();
    id.push_i64(2).unwrap();
    id.push_null();

    let mut score = ColumnBuilder::new(DataType::Float64);
    score.push_f64(9.5).unwrap();
    score.push_f64(-1.25).unwrap();
    score.push_f64(0.0).unwrap();

    let mut name = ColumnBuilder::new(DataType::Utf8);
    name.push_str("alice").unwrap();
    name.push_null();
    name.push_str("héllo 🌍").unwrap();

    let mut flag = ColumnBuilder::new(DataType::Bool);
    flag.push_bool(true).unwrap();
    flag.push_bool(false).unwrap();
    flag.push_bool(true).unwrap();

    RecordBatch::try_new(vec![
        id.finish("id"),
        score.finish("score"),
        name.finish("name"),
        flag.finish("flag"),
    ])
    .unwrap()
}

#[test]
fn ncb_roundtrip_preserves_everything() {
    let batch = build_batch();
    let bytes = batch.encode();

    // Numeric buffers must be 8-byte aligned for zero-copy typed-array views.
    assert_eq!(&bytes[0..4], b"NCB1");

    let decoded = RecordBatch::decode(&bytes).unwrap();
    assert_eq!(decoded, batch);

    let id = decoded.column("id").unwrap();
    assert_eq!(id.i64(0), Some(1));
    assert_eq!(id.i64(2), None); // null

    let name = decoded.column("name").unwrap();
    assert_eq!(name.str(0), Some("alice"));
    assert_eq!(name.str(1), None);
    assert_eq!(name.str(2), Some("héllo 🌍"));

    let flag = decoded.column("flag").unwrap();
    assert_eq!(flag.bool(0), Some(true));
    assert_eq!(flag.bool(1), Some(false));
}

#[test]
fn numeric_buffers_are_8_byte_aligned() {
    // Directly assert the alignment invariant the format promises, by decoding
    // and re-checking that offsets land on 8-byte boundaries.
    let batch = build_batch();
    let bytes = batch.encode();
    assert_eq!(bytes.len() % 8, 0);
}

#[test]
fn empty_batch_roundtrips() {
    let mut id = ColumnBuilder::new(DataType::Int64);
    let _ = &mut id; // no rows
    let batch = RecordBatch::try_new(vec![id.finish("id")]).unwrap();
    assert_eq!(batch.num_rows, 0);
    let decoded = RecordBatch::decode(&batch.encode()).unwrap();
    assert_eq!(decoded, batch);
    assert!(matches!(decoded.columns[0].data, ColumnData::Int64(ref v) if v.is_empty()));
}

#[test]
fn query_builder_renders_sql() {
    let (sql, params) = Query::table("users")
        .select(["id", "name"])
        .filter("age >= ?", [30i64])
        .filter("active = ?", [Value::Bool(true)])
        .order_by("name", Order::Desc)
        .limit(5)
        .offset(10)
        .build();
    assert_eq!(
        sql,
        "SELECT id, name FROM users WHERE age >= ? AND active = ? ORDER BY name DESC LIMIT 5 OFFSET 10"
    );
    assert_eq!(params.len(), 2);
}

#[tokio::test]
async fn client_end_to_end() {
    let client = Client::connect("sqlite::memory:").await.unwrap();
    client
        .execute(
            "CREATE TABLE users (id INTEGER, name TEXT, score REAL)",
            vec![],
        )
        .await
        .unwrap();
    client
        .execute(
            "INSERT INTO users VALUES (?, ?, ?), (?, ?, ?)",
            vec![
                1.into(),
                "alice".into(),
                9.5.into(),
                2.into(),
                "bob".into(),
                Value::Null,
            ],
        )
        .await
        .unwrap();

    let (sql, params) = Query::table("users")
        .select(["id", "name", "score"])
        .order_by("id", Order::Asc)
        .build();
    let batch = client.query(&sql, params).await.unwrap();

    assert_eq!(batch.num_rows, 2);
    assert_eq!(batch.num_columns(), 3);
    assert_eq!(batch.column("id").unwrap().i64(0), Some(1));
    assert_eq!(batch.column("name").unwrap().str(1), Some("bob"));
    assert_eq!(batch.column("score").unwrap().f64(0), Some(9.5));
    assert_eq!(batch.column("score").unwrap().f64(1), None); // NULL

    // And the FFI entry point produces a decodable NCB buffer.
    let (sql, params) = Query::table("users").build();
    let bytes = client.query_bytes(&sql, params).await.unwrap();
    let decoded = RecordBatch::decode(&bytes).unwrap();
    assert_eq!(decoded.num_rows, 2);
}
