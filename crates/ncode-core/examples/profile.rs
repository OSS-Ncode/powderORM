//! Component-level profiling for the bench-site query shape:
//! 200k rows of (id INTEGER, name TEXT, score REAL), `SELECT ... ORDER BY id`.
//!
//! Run: cargo run -p ncode-core --example profile --release

use std::time::Instant;

use ncode_core::Client;
use rusqlite::Connection;

const ROWS: usize = 200_000;
const CHUNK: usize = 500;

fn seed(conn: &Connection) {
    conn.execute_batch("CREATE TABLE bench_users (id INTEGER, name TEXT, score REAL)")
        .unwrap();
    let mut i = 0usize;
    while i < ROWS {
        let end = (i + CHUNK).min(ROWS);
        let mut sql = String::from("INSERT INTO bench_users (id, name, score) VALUES ");
        for r in i..end {
            if r > i {
                sql.push(',');
            }
            let name = format!("user_{}_{}", r, (r * 2654435761) % 1000);
            let score = (((r * 37) % 10000) as f64 / 7.0).round() / 10.0;
            sql.push_str(&format!("({}, '{}', {})", r + 1, name, score));
        }
        conn.execute_batch(&sql).unwrap();
        i = end;
    }
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn time_n<F: FnMut() -> ()>(n: usize, mut f: F) -> f64 {
    let mut samples = Vec::with_capacity(n);
    for _ in 0..n {
        let t = Instant::now();
        f();
        samples.push(t.elapsed().as_secs_f64() * 1e3);
    }
    median(samples)
}

fn main() {
    let conn = Connection::open_in_memory().unwrap();
    seed(&conn);

    const SQL: &str = "SELECT id, name, score FROM bench_users ORDER BY id ASC";
    const SQL_NOSORT: &str = "SELECT id, name, score FROM bench_users";

    // 1. step-only floor: iterate all rows, never touch columns.
    let step_only = time_n(7, || {
        let mut stmt = conn.prepare(SQL).unwrap();
        let mut rows = stmt.query([]).unwrap();
        let mut n = 0usize;
        while let Some(_row) = rows.next().unwrap() {
            n += 1;
        }
        assert_eq!(n, ROWS);
    });

    // 2. step + get_ref every column (what run_query's input side costs).
    let step_read = time_n(7, || {
        let mut stmt = conn.prepare(SQL).unwrap();
        let mut rows = stmt.query([]).unwrap();
        let mut acc = 0i64;
        while let Some(row) = rows.next().unwrap() {
            for i in 0..3 {
                if let rusqlite::types::ValueRef::Integer(v) = row.get_ref(i).unwrap() {
                    acc ^= v;
                }
            }
        }
        std::hint::black_box(acc);
    });

    // 3. same, without ORDER BY — isolates the sorter.
    let step_read_nosort = time_n(7, || {
        let mut stmt = conn.prepare(SQL_NOSORT).unwrap();
        let mut rows = stmt.query([]).unwrap();
        let mut n = 0usize;
        while let Some(row) = rows.next().unwrap() {
            for i in 0..3 {
                std::hint::black_box(row.get_ref(i).unwrap());
            }
            n += 1;
        }
        assert_eq!(n, ROWS);
    });

    // 4/5. full pipeline through the public client: query (build) then encode.
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let client = Client::connect(":memory:").await.unwrap();
        // Re-seed through a raw handle is not possible via Client, so seed with SQL.
        client
            .execute(
                "CREATE TABLE bench_users (id INTEGER, name TEXT, score REAL)",
                vec![],
            )
            .await
            .unwrap();
        let mut i = 0usize;
        while i < ROWS {
            let end = (i + CHUNK).min(ROWS);
            let mut sql = String::from("INSERT INTO bench_users (id, name, score) VALUES ");
            for r in i..end {
                if r > i {
                    sql.push(',');
                }
                let name = format!("user_{}_{}", r, (r * 2654435761) % 1000);
                let score = (((r * 37) % 10000) as f64 / 7.0).round() / 10.0;
                sql.push_str(&format!("({}, '{}', {})", r + 1, name, score));
            }
            client.execute(&sql, vec![]).await.unwrap();
            i = end;
        }

        let mut q_samples = Vec::new();
        let mut e_samples = Vec::new();
        let mut qb_samples = Vec::new();
        for _ in 0..7 {
            let t = Instant::now();
            let batch = client.query(SQL, vec![]).await.unwrap();
            q_samples.push(t.elapsed().as_secs_f64() * 1e3);

            let t = Instant::now();
            let bytes = batch.encode();
            e_samples.push(t.elapsed().as_secs_f64() * 1e3);
            std::hint::black_box(bytes.len());

            let t = Instant::now();
            let bytes = client.query_bytes(SQL, vec![]).await.unwrap();
            qb_samples.push(t.elapsed().as_secs_f64() * 1e3);
            std::hint::black_box(bytes.len());
        }
        println!("step_only (sort+step, no reads) : {step_only:8.2} ms");
        println!("step+get_ref x3                 : {step_read:8.2} ms");
        println!("step+get_ref x3, NO sort        : {step_read_nosort:8.2} ms");
        println!("client.query (build batch)      : {:8.2} ms", median(q_samples));
        println!("batch.encode                    : {:8.2} ms", median(e_samples));
        println!("client.query_bytes (e2e native) : {:8.2} ms", median(qb_samples));
    });
}
