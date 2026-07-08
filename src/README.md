# Ncode

A high-performance database engine with a **Rust core** that returns query
results in a **zero-copy, Apache-Arrow-style columnar binary format**, exposed
to **TypeScript** (napi-rs) and **Python** (PyO3) through idiomatic, fully
`async` APIs.

```
          ┌────────────────────────────────────────────┐
          │            ncode-core  (Rust)               │
          │  Client · Query builder · RecordBatch        │
          │  NCB columnar codec (zero-copy)              │
          │  async engine (Tokio) → rusqlite backend     │
          └───────────────┬───────────────┬─────────────┘
                          │               │
              napi-rs     │               │   PyO3 + pyo3-async-runtimes
         ┌────────────────▼──┐         ┌──▼──────────────────┐
         │   @ncode/node     │         │      ncode (py)      │
         │  Promise · TS      │         │  asyncio · typing    │
         │  typed-array reader│         │  memoryview reader   │
         └────────────────────┘         └──────────────────────┘
```

## Why

Moving relational result sets across a language boundary usually means
serializing to JSON or building millions of host-language objects. Ncode
instead moves **one contiguous columnar buffer** (the *NCB* format) and lets the
host language build typed-array / `memoryview` views straight over those bytes —
so a `Float64Array` in Node or a `memoryview.cast('d')` in Python reads the
engine's output with **no per-value copy**.

## Layout

| Crate / package        | Role                                                        |
| ---------------------- | ----------------------------------------------------------- |
| `crates/ncode-core`    | Rust core: async client, query builder, NCB codec           |
| `crates/ncode-node`    | napi-rs binding + TypeScript wrapper (`@ncode/node`)         |
| `crates/ncode-python`  | PyO3 binding + pure-Python wrapper (`ncode`)                 |

The wire format is specified in [`docs/FORMAT.md`](docs/FORMAT.md).

## Rust

```rust
use ncode_core::{Client, query::Query, query::Order};

# async fn demo() -> ncode_core::Result<()> {
let db = Client::connect("sqlite::memory:").await?;
db.execute("CREATE TABLE users (id INTEGER, name TEXT, score REAL)", vec![]).await?;
db.execute(
    "INSERT INTO users VALUES (?, ?, ?)",
    vec![1.into(), "alice".into(), 9.5.into()],
).await?;

let (sql, params) = Query::table("users")
    .select(["id", "name", "score"])
    .filter("score > ?", [5.0])
    .order_by("id", Order::Asc)
    .build();

let batch = db.query(&sql, params).await?;
println!("{} rows", batch.num_rows);
println!("first name = {:?}", batch.column("name").unwrap().str(0));
# Ok(())
# }
```

```bash
cargo test -p ncode-core        # runs the core unit + integration tests
```

## Node.js / TypeScript

```ts
import { Client, Query } from "@ncode/node";

const db = await Client.connect("sqlite::memory:");
await db.execute("CREATE TABLE users (id INTEGER, name TEXT, score REAL)");
await db.execute("INSERT INTO users VALUES (?, ?, ?)", [1, "alice", 9.5]);

const batch = await db.run(
  Query.table("users").select("id", "name", "score").filter("score > ?", 5),
);

// Zero-copy typed-array view over the engine's output buffer:
const score = batch.column("score")!;        // Float64Array-backed
console.log(score.get(0));                    // 9.5
console.log(batch.toRows());                  // [{ id: 1n, name: "alice", score: 9.5 }]
```

Build the native addon + types:

```bash
cd crates/ncode-node
npm install
npm run build        # napi build --release && tsc
```

## Python

```python
import asyncio, ncode

async def main():
    db = await ncode.connect("sqlite::memory:")
    await db.execute("CREATE TABLE users (id INTEGER, name TEXT, score REAL)")
    await db.execute("INSERT INTO users VALUES (?, ?, ?)", [1, "alice", 9.5])

    batch = await db.run(
        ncode.Query.table("users").select("id", "name", "score").filter("score > ?", 5)
    )
    # Zero-copy memoryview over the engine's output buffer:
    print(batch.column("score").get(0))   # 9.5
    print(batch.to_rows())                 # [{'id': 1, 'name': 'alice', 'score': 9.5}]

asyncio.run(main())
```

Build & install the extension:

```bash
cd crates/ncode-python
python -m venv .venv && source .venv/bin/activate
pip install maturin
maturin develop          # builds the Rust extension and installs `ncode`
```

## Supported types

`Int64`, `Float64`, `Bool`, and `Utf8` — each nullable via a validity bitmap.

## License

MIT
