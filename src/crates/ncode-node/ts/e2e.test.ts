import test from "node:test";
import assert from "node:assert/strict";
import { Client, Query, DataType } from "./index.js";

test("node end-to-end: async engine + zero-copy reader", async () => {
  const db = await Client.connect("sqlite::memory:");
  await db.execute("CREATE TABLE users (id INTEGER, name TEXT, score REAL)");
  const affected = await db.execute(
    "INSERT INTO users VALUES (?,?,?),(?,?,?),(?,?,?)",
    [1, "alice", 9.5, 2, "bob", null, 3, "héllo 🌍", -1.25],
  );
  assert.equal(affected, 3);

  const batch = await db.run(
    Query.table("users").select("id", "name", "score").order("id", "ASC"),
  );

  assert.equal(batch.numRows, 3);
  assert.deepEqual(
    batch.columns.map((c) => c.name),
    ["id", "name", "score"],
  );

  const ids = batch.column("id")!;
  assert.equal(ids.type, DataType.Int64);
  assert.deepEqual(ids.toArray(), [1n, 2n, 3n]); // int64 -> BigInt

  const names = batch.column("name")!;
  assert.equal(names.type, DataType.Utf8);
  assert.deepEqual(names.toArray(), ["alice", "bob", "héllo 🌍"]);

  const scores = batch.column("score")!;
  assert.equal(scores.type, DataType.Float64);
  assert.equal(scores.get(0), 9.5);
  assert.equal(scores.get(1), null); // NULL preserved via validity bitmap
  assert.equal(scores.get(2), -1.25);

  assert.deepEqual(batch.toRows()[2], {
    id: 3n,
    name: "héllo 🌍",
    score: -1.25,
  });
});
