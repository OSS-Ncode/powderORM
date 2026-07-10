import { test } from "node:test";
import assert from "node:assert/strict";
import {
  Client,
  PowderError,
  PowderTable,
  runNamedQuery,
  type TableMeta,
  type Where,
} from "./index.js";

interface Post {
  id: number;
  user_id: number;
  title: string;
  user?: User | null;
}

const POSTS_META: TableMeta = {
  table: "posts",
  columns: [
    { name: "id", type: "int", primaryKey: true },
    { name: "user_id", type: "int" },
    { name: "title", type: "text" },
  ],
  sql: {
    selectAll: "SELECT id, user_id, title FROM posts",
    insert: "INSERT INTO posts (id, user_id, title) VALUES (?, ?, ?)",
    countAll: "SELECT COUNT(*) AS n FROM posts",
    deleteAll: "DELETE FROM posts",
    ident: { id: "id", user_id: "user_id", title: "title" },
  },
  relations: [
    {
      name: "user",
      kind: "belongsTo",
      localColumns: ["user_id"],
      foreignColumns: ["id"],
      target: () => USERS_META,
    },
  ],
};

interface User {
  id: number;
  name: string | null;
  score: number | null;
  active: boolean;
  posts?: Post[];
}

// The shape `powder generate` emits for a table.
const USERS_META: TableMeta = {
  table: "users",
  columns: [
    { name: "id", type: "int", primaryKey: true },
    { name: "name", type: "text", nullable: true },
    { name: "score", type: "float", nullable: true },
    { name: "active", type: "bool" },
  ],
  sql: {
    selectAll: "SELECT id, name, score, active FROM users",
    insert: "INSERT INTO users (id, name, score, active) VALUES (?, ?, ?, ?)",
    countAll: "SELECT COUNT(*) AS n FROM users",
    deleteAll: "DELETE FROM users",
    ident: { id: "id", name: "name", score: "score", active: "active" },
  },
  relations: [
    {
      name: "posts",
      kind: "hasMany",
      localColumns: ["id"],
      foreignColumns: ["user_id"],
      target: () => POSTS_META,
    },
  ],
};

async function setup(): Promise<PowderTable<User>> {
  const db = await Client.connect("sqlite::memory:");
  await db.execute(
    "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, score REAL, active INTEGER NOT NULL)",
  );
  return new PowderTable<User>(db, USERS_META);
}

test("create / findMany / findFirst round-trip with typed coercion", async () => {
  const users = await setup();
  await users.create({ id: 1, name: "alice", score: 9.5, active: true });
  await users.create({ id: 2, name: "bob", score: null, active: false });
  await users.create({ id: 3, name: null, score: 3.25, active: true });

  const all = await users.findMany({ orderBy: { id: "asc" } });
  assert.equal(all.length, 3);
  assert.deepEqual(all[0], { id: 1, name: "alice", score: 9.5, active: true });
  assert.deepEqual(all[1], { id: 2, name: "bob", score: null, active: false });
  assert.equal(typeof all[0].id, "number"); // bigint coerced to number

  const bob = await users.findFirst({ where: { name: "bob" } });
  assert.equal(bob?.id, 2);
  assert.equal(await users.findFirst({ where: { name: "nobody" } }), null);
});

test("where operators compile correctly", async () => {
  const users = await setup();
  await users.createMany([
    { id: 1, name: "a", score: 1, active: true },
    { id: 2, name: "b", score: 2, active: false },
    { id: 3, name: "c", score: 3, active: true },
    { id: 4, name: null, score: null, active: false },
  ]);

  assert.equal((await users.findMany({ where: { score: { gte: 2 } } })).length, 2);
  assert.equal((await users.findMany({ where: { score: { gt: 1, lt: 3 } } })).length, 1);
  assert.equal((await users.findMany({ where: { id: { in: [1, 3] } } })).length, 2);
  assert.equal((await users.findMany({ where: { id: { in: [] } } })).length, 0);
  assert.equal((await users.findMany({ where: { name: null } })).length, 1);
  assert.equal((await users.findMany({ where: { name: { ne: null } } })).length, 3);
  assert.equal((await users.findMany({ where: { name: { like: "a%" } } })).length, 1);
  assert.equal(await users.count(), 4);
  assert.equal(await users.count({ active: true }), 2);
});

test("update and delete return affected counts", async () => {
  const users = await setup();
  await users.createMany([
    { id: 1, name: "a", score: 1, active: true },
    { id: 2, name: "b", score: 2, active: true },
  ]);

  assert.equal(await users.update({ where: { id: 1 }, data: { score: 10 } }), 1);
  assert.equal((await users.findFirst({ where: { id: 1 } }))?.score, 10);

  assert.equal(await users.delete({ id: 2 }), 1);
  assert.equal(await users.count(), 1);

  await assert.rejects(() => users.delete({}), PowderError);
  assert.equal(await users.deleteAll(), 1);
  assert.equal(await users.count(), 0);
});

test("transaction commits on return and rolls back on throw", async () => {
  const db = await Client.connect("sqlite::memory:");
  await db.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL)");

  await db.transaction(async (tx) => {
    await tx.execute("INSERT INTO t VALUES (1, 'a')");
    await tx.execute("INSERT INTO t VALUES (2, 'b')");
  });
  let batch = await db.query("SELECT COUNT(*) AS n FROM t");
  assert.equal(batch.column("n")!.get(0), 2);

  await assert.rejects(
    db.transaction(async (tx) => {
      await tx.execute("INSERT INTO t VALUES (3, 'c')");
      throw new Error("boom");
    }),
    /boom/,
  );
  batch = await db.query("SELECT COUNT(*) AS n FROM t");
  assert.equal(batch.column("n")!.get(0), 2); // rolled back
});

test("include batch-loads foreign-key relations", async () => {
  const client = await Client.connect("sqlite::memory:");
  await client.execute(
    "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, score REAL, active INTEGER NOT NULL)",
  );
  await client.execute(
    "CREATE TABLE posts (id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL REFERENCES users(id), title TEXT NOT NULL)",
  );
  const users = new PowderTable<User>(client, USERS_META);
  const posts = new PowderTable<Post>(client, POSTS_META);

  await users.createMany([
    { id: 1, name: "alice", score: 9.5, active: true },
    { id: 2, name: "bob", score: null, active: false },
  ]);
  await posts.createMany([
    { id: 1, user_id: 1, title: "hello" },
    { id: 2, user_id: 1, title: "again" },
    { id: 3, user_id: 2, title: "hi" },
  ]);

  // belongsTo via include (2 queries, no N+1).
  const rows = await posts.findMany({ include: { user: true }, orderBy: { id: "asc" } });
  assert.equal(rows.length, 3);
  assert.equal(rows[0].user?.name, "alice");
  assert.equal(rows[1].user?.name, "alice");
  assert.equal(rows[2].user?.name, "bob");

  // belongsTo via a single LEFT JOIN query — same result.
  const joined = await posts.findMany({ join: { user: true }, orderBy: { id: "asc" } });
  assert.equal(joined[0].user?.name, "alice");
  assert.equal(joined[2].user?.name, "bob");

  // hasMany reverse include: each user gets its posts array.
  const withPosts = await users.findMany({ include: { posts: true }, orderBy: { id: "asc" } });
  assert.deepEqual(withPosts[0].posts?.map((p) => p.title), ["hello", "again"]);
  assert.deepEqual(withPosts[1].posts?.map((p) => p.title), ["hi"]);

  // create() must ignore relation fields present on a fetched row.
  const roundTrip = { ...withPosts[0], id: 9, name: "eve" };
  await users.create(roundTrip);
  assert.equal(await users.count(), 3);

  // hasMany cannot be joined (it would multiply parent rows).
  await assert.rejects(
    () => users.findMany({ join: { posts: true } }),
    (e: PowderError) => e instanceof PowderError && /hasMany/.test(e.message),
  );

  // Unknown relation names fail fast.
  await assert.rejects(
    () => posts.findMany({ include: { ghost: true } }),
    (e: PowderError) => e instanceof PowderError && /unknown relation/.test(e.message),
  );
});

test("composite foreign-key relation loads by tuple", async () => {
  const client = await Client.connect("sqlite::memory:");
  await client.execute(
    "CREATE TABLE orders (id INTEGER NOT NULL, year INTEGER NOT NULL, total REAL, PRIMARY KEY (id, year))",
  );
  await client.execute(
    "CREATE TABLE line_items (id INTEGER PRIMARY KEY, order_id INTEGER NOT NULL, order_year INTEGER NOT NULL, sku TEXT NOT NULL, FOREIGN KEY (order_id, order_year) REFERENCES orders(id, year))",
  );
  interface Order { id: number; year: number; total: number | null }
  interface LineItem {
    id: number;
    order_id: number;
    order_year: number;
    sku: string;
    order?: Order | null;
  }
  const ORDERS_META: TableMeta = {
    table: "orders",
    columns: [
      { name: "id", type: "int", primaryKey: true },
      { name: "year", type: "int", primaryKey: true },
      { name: "total", type: "float", nullable: true },
    ],
    sql: {
      selectAll: "SELECT id, year, total FROM orders",
      insert: "INSERT INTO orders (id, year, total) VALUES (?, ?, ?)",
      countAll: "SELECT COUNT(*) AS n FROM orders",
      deleteAll: "DELETE FROM orders",
      ident: { id: "id", year: "year", total: "total" },
    },
  };
  const ITEMS_META: TableMeta = {
    table: "line_items",
    columns: [
      { name: "id", type: "int", primaryKey: true },
      { name: "order_id", type: "int" },
      { name: "order_year", type: "int" },
      { name: "sku", type: "text" },
    ],
    sql: {
      selectAll: "SELECT id, order_id, order_year, sku FROM line_items",
      insert: "INSERT INTO line_items (id, order_id, order_year, sku) VALUES (?, ?, ?, ?)",
      countAll: "SELECT COUNT(*) AS n FROM line_items",
      deleteAll: "DELETE FROM line_items",
      ident: { id: "id", order_id: "order_id", order_year: "order_year", sku: "sku" },
    },
    relations: [
      {
        name: "order",
        kind: "belongsTo",
        localColumns: ["order_id", "order_year"],
        foreignColumns: ["id", "year"],
        target: () => ORDERS_META,
      },
    ],
  };
  const orders = new PowderTable<Order>(client, ORDERS_META);
  const items = new PowderTable<LineItem>(client, ITEMS_META);
  await orders.createMany([
    { id: 1, year: 2026, total: 100 },
    { id: 1, year: 2025, total: 50 }, // same id, different year
  ]);
  await items.createMany([
    { id: 1, order_id: 1, order_year: 2026, sku: "A" },
    { id: 2, order_id: 1, order_year: 2025, sku: "B" },
  ]);

  // The tuple (order_id, order_year) must disambiguate the same id.
  const viaInclude = await items.findMany({ include: { order: true }, orderBy: { id: "asc" } });
  assert.equal(viaInclude[0].order?.total, 100);
  assert.equal(viaInclude[1].order?.total, 50);

  const viaJoin = await items.findMany({ join: { order: true }, orderBy: { id: "asc" } });
  assert.equal(viaJoin[0].order?.total, 100);
  assert.equal(viaJoin[1].order?.total, 50);
});

test("nested include recurses one batched query per level", async () => {
  const client = await Client.connect("sqlite::memory:");
  await client.execute(
    "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, score REAL, active INTEGER NOT NULL)",
  );
  await client.execute(
    "CREATE TABLE posts (id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL REFERENCES users(id), title TEXT NOT NULL)",
  );
  const users = new PowderTable<User>(client, USERS_META);
  const posts = new PowderTable<Post>(client, POSTS_META);
  await users.createMany([
    { id: 1, name: "alice", score: 9.5, active: true },
    { id: 2, name: "bob", score: 1, active: false },
  ]);
  await posts.createMany([
    { id: 1, user_id: 1, title: "hello" },
    { id: 2, user_id: 1, title: "again" },
    { id: 3, user_id: 2, title: "hi" },
  ]);

  // posts -> user -> posts (belongsTo then hasMany, two levels deep).
  const rows = await posts.findMany({
    include: { user: { include: { posts: true } } },
    orderBy: { id: "asc" },
  });
  assert.equal(rows[0].user?.name, "alice");
  assert.deepEqual(rows[0].user?.posts?.map((p) => p.title), ["hello", "again"]);
  assert.deepEqual(rows[2].user?.posts?.map((p) => p.title), ["hi"]);

  // An unknown relation at a nested level still fails fast.
  await assert.rejects(
    () => posts.findMany({ include: { user: { include: { ghost: true } } } }),
    (e: PowderError) => e instanceof PowderError && /unknown relation/.test(e.message),
  );
});

test("beginner API: find(), where().orderBy().limit().all()", async () => {
  const users = await setup();
  await users.createMany([
    { id: 1, name: "alice", score: 9.5, active: true },
    { id: 2, name: "bob", score: 2, active: true },
    { id: 3, name: "carol", score: 7, active: false },
  ]);

  // find() by single-column primary key.
  assert.equal((await users.find(2))?.name, "bob");
  assert.equal(await users.find(99), null);
  // find() with an object works for any lookup.
  assert.equal((await users.find({ name: "carol" }))?.id, 3);

  // Chainable finder; each step returns a fresh Finder.
  const base = users.where({ active: true });
  const top = await base.orderBy("score", "desc").limit(1).all();
  assert.deepEqual(top.map((u) => u.name), ["alice"]);
  assert.equal(await base.count(), 2);
  assert.equal((await base.orderBy("score", "asc").first())?.name, "bob");
  // The shared `base` was not mutated by the chains above.
  assert.equal((await base.all()).length, 2);

  // where() merges; later calls override the same column.
  assert.equal(await users.where({ active: true }).where({ active: false }).count(), 1);

  // all() on the table is every row.
  assert.equal((await users.all()).length, 3);
});

test("find(value) rejects composite primary keys with a clear message", async () => {
  const client = await Client.connect("sqlite::memory:");
  await client.execute(
    "CREATE TABLE grades (student INTEGER NOT NULL, course TEXT NOT NULL, PRIMARY KEY (student, course))",
  );
  interface Grade { student: number; course: string }
  const META: TableMeta = {
    table: "grades",
    columns: [
      { name: "student", type: "int", primaryKey: true },
      { name: "course", type: "text", primaryKey: true },
    ],
    sql: {
      selectAll: "SELECT student, course FROM grades",
      insert: "INSERT INTO grades (student, course) VALUES (?, ?)",
      countAll: "SELECT COUNT(*) AS n FROM grades",
      deleteAll: "DELETE FROM grades",
      ident: { student: "student", course: "course" },
    },
  };
  const grades = new PowderTable<Grade>(client, META);
  await grades.create({ student: 1, course: "math" });
  await assert.rejects(
    () => grades.find(1),
    (e: PowderError) => e instanceof PowderError && /composite primary key/.test(e.message),
  );
  assert.equal((await grades.find({ student: 1, course: "math" }))?.course, "math");
});

test("named query binds params by name and types rows", async () => {
  const client = await Client.connect("sqlite::memory:");
  await client.execute(
    "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, score REAL, active INTEGER NOT NULL)",
  );
  const users = new PowderTable<User>(client, USERS_META);
  await users.createMany([
    { id: 1, name: "alice", score: 9.5, active: true },
    { id: 2, name: "bob", score: 2, active: true },
    { id: 3, name: "carol", score: 8, active: false },
  ]);

  // Shape emitted by `powder generate` for a schema `queries` entry.
  const SQL = "SELECT id, name, score, active FROM users WHERE active = ? AND score >= ? ORDER BY score DESC";
  const rows = await runNamedQuery(client, SQL, ["active", "minScore"], { active: true, minScore: 5 }, USERS_META);
  assert.deepEqual(rows.map((r) => r.name), ["alice"]);
  assert.equal(typeof rows[0].active, "boolean"); // typed via meta

  // A param used twice binds twice, in order.
  const twice = await runNamedQuery(
    client,
    "SELECT id FROM users WHERE id > ? OR id < ?",
    ["x", "x"],
    { x: 2 },
    undefined,
  );
  assert.deepEqual(twice.map((r) => Number(r.id)).sort(), [1, 3]);

  // Missing arguments fail fast with the query in the error.
  await assert.rejects(
    () => runNamedQuery(client, SQL, ["active", "minScore"], { active: true }, USERS_META),
    (e: PowderError) => e instanceof PowderError && /missing parameter `minScore`/.test(e.message),
  );
});

test("where-shape cache reuses SQL but never reuses values", async () => {
  const users = await setup();
  await users.createMany([
    { id: 1, name: "alice", score: 9.5, active: true },
    { id: 2, name: "bob", score: 2, active: true },
    { id: 3, name: "carol", score: 7, active: false },
  ]);

  // Same shape, different values -> same SQL, different rows.
  assert.deepEqual((await users.findMany({ where: { id: 1 } })).map((u) => u.name), ["alice"]);
  assert.deepEqual((await users.findMany({ where: { id: 2 } })).map((u) => u.name), ["bob"]);

  // Same shape, different operator values.
  assert.equal((await users.findMany({ where: { score: { gte: 5 } } })).length, 2);
  assert.equal((await users.findMany({ where: { score: { gte: 100 } } })).length, 0);

  // `IN` arity is part of the shape: differing lengths must not share SQL.
  assert.equal((await users.findMany({ where: { id: { in: [1] } } })).length, 1);
  assert.equal((await users.findMany({ where: { id: { in: [1, 3] } } })).length, 2);
  assert.equal((await users.findMany({ where: { id: { in: [] } } })).length, 0);

  // Null-ness is part of the shape: `eq: null` has no bound parameter.
  await users.create({ id: 4, name: null, score: null, active: true });
  assert.equal((await users.findMany({ where: { name: null } })).length, 1);
  assert.equal((await users.findMany({ where: { name: { ne: null } } })).length, 3);
  assert.equal((await users.findMany({ where: { name: "alice" } })).length, 1);

  // Mixed multi-column predicates round-trip correctly after caching.
  const q = { where: { active: true, score: { gte: 2, lt: 9 } } } as const;
  assert.deepEqual((await users.findMany(q)).map((u) => u.name), ["bob"]);
  assert.deepEqual(
    (await users.findMany({ where: { active: true, score: { gte: 9, lt: 10 } } })).map((u) => u.name),
    ["alice"],
  );
});

test("nested transactions use savepoints", async () => {
  const db = await Client.connect("sqlite::memory:");
  await db.execute("CREATE TABLE t (id INTEGER PRIMARY KEY)");

  // Inner rolls back, outer commits: only rows outside the failed inner survive.
  await db.transaction(async (tx) => {
    await tx.execute("INSERT INTO t VALUES (1)");
    await tx
      .transaction(async (inner) => {
        await inner.execute("INSERT INTO t VALUES (2)");
        throw new Error("inner boom");
      })
      .catch(() => {});
    await tx.execute("INSERT INTO t VALUES (3)");
  });
  let rows = (await db.query("SELECT id FROM t ORDER BY id")).column("id")!;
  assert.deepEqual([rows.get(0), rows.get(1)], [1, 3]); // 2 was rolled back

  // Inner commits, outer rolls back: everything is undone.
  await db.execute("DELETE FROM t");
  await db
    .transaction(async (tx) => {
      await tx.transaction(async (inner) => {
        await inner.execute("INSERT INTO t VALUES (9)");
      });
      throw new Error("outer boom");
    })
    .catch(() => {});
  const n = (await db.query("SELECT COUNT(*) AS n FROM t")).column("n")!.get(0);
  assert.equal(n, 0);
});

test("composite primary keys work through the ORM", async () => {
  const client = await Client.connect("sqlite::memory:");
  await client.execute(
    "CREATE TABLE grades (student INTEGER NOT NULL, course TEXT NOT NULL, grade REAL, PRIMARY KEY (student, course))",
  );
  interface Grade {
    student: number;
    course: string;
    grade: number | null;
  }
  const META: TableMeta = {
    table: "grades",
    columns: [
      { name: "student", type: "int", primaryKey: true },
      { name: "course", type: "text", primaryKey: true },
      { name: "grade", type: "float", nullable: true },
    ],
    sql: {
      selectAll: "SELECT student, course, grade FROM grades",
      insert: "INSERT INTO grades (student, course, grade) VALUES (?, ?, ?)",
      countAll: "SELECT COUNT(*) AS n FROM grades",
      deleteAll: "DELETE FROM grades",
      ident: { student: "student", course: "course", grade: "grade" },
    },
  };
  const grades = new PowderTable<Grade>(client, META);
  await grades.create({ student: 1, course: "math", grade: 4.0 });
  await grades.create({ student: 1, course: "art", grade: 3.5 });
  // The composite key is enforced by the database and surfaces as PowderError.
  await assert.rejects(
    () => grades.create({ student: 1, course: "math", grade: 2.0 }),
    PowderError,
  );
  const row = await grades.findFirst({ where: { student: 1, course: "math" } });
  assert.equal(row?.grade, 4.0);
  assert.equal(await grades.count(), 2);
});

test("PowderError carries SQL and a clickable caller site", async () => {
  const users = await setup();
  // Force a real DB error: duplicate primary key.
  await users.create({ id: 1, name: "x", score: 0, active: true });
  let caught: PowderError | undefined;
  try {
    await users.create({ id: 1, name: "y", score: 0, active: true });
  } catch (e) {
    caught = e as PowderError;
  }
  assert.ok(caught instanceof PowderError);
  assert.match(caught.sql, /INSERT INTO users/);
  // The site must point at THIS test file, not the ORM internals.
  assert.ok(caught.site, "expected a captured call site");
  assert.match(caught.site!, /orm\.test\.(ts|js):\d+:\d+$/);
  assert.match(caught.message, /at .*orm\.test\.(ts|js):\d+:\d+/);

  // Unknown columns are caught before touching the database.
  await assert.rejects(
    () => users.findMany({ where: { ghost: 1 } as never }),
    (e: PowderError) => e instanceof PowderError && /unknown column/.test(e.message),
  );
});

test("beginner syntax: 3-arg where, exists, pluck, aggregates, paginate", async () => {
  const users = await setup();
  await users.createMany([
    { id: 1, name: "alice", score: 9.5, active: true },
    { id: 2, name: "bob", score: 4.0, active: false },
    { id: 3, name: "carol", score: 7.0, active: true },
    { id: 4, name: "dave", score: null, active: true },
  ]);

  // 3-arg where on the table and on the finder; merges with object form.
  assert.deepEqual(
    (await users.where("score", ">=", 5).orderBy("id").pluck("name")),
    ["alice", "carol"],
  );
  assert.equal(await users.where("name", "like", "a%").count(), 1);
  assert.equal(
    await users.where({ active: true }).where("score", ">", 8).count(),
    1,
  );
  assert.equal(await users.where("id", "in", [1, 3]).count(), 2);

  // exists
  assert.equal(await users.where("score", ">", 100).exists(), false);
  assert.equal(await users.exists({ active: true }), true);

  // aggregates (NULL score is ignored by SQL aggregate semantics)
  assert.equal(await users.where({ active: true }).sum("score"), 16.5);
  assert.equal(await users.where({ active: true }).avg("score"), 8.25);
  assert.equal(await users.where({}).min("score"), 4.0);
  assert.equal(await users.where({}).max("score"), 9.5);
  assert.equal(await users.where("id", ">", 99).sum("score"), null);

  // unknown operator fails loudly
  await assert.rejects(
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    async () => users.where("score", "=~" as any, 1).all(),
    PowderError,
  );

  // paginate: 1-based, returns total across all pages
  const page1 = await users.orderBy("id").paginate(1, 3);
  assert.deepEqual(page1.rows.map((r) => r.id), [1, 2, 3]);
  assert.equal(page1.total, 4);
  assert.equal(page1.totalPages, 2);
  const page2 = await users.orderBy("id").paginate(2, 3);
  assert.deepEqual(page2.rows.map((r) => r.id), [4]);
});

test("createMany safeguards: variable-limit clamp and shape validation", async () => {
  const users = await setup();

  // A chunk size that would exceed SQLite's bound-variable ceiling is
  // clamped internally instead of failing (4 columns x 20000 rows/chunk
  // would be 80k variables).
  const many: Array<Partial<User>> = [];
  for (let i = 1; i <= 12_000; i++) {
    many.push({ id: i, name: `u${i}`, score: i / 10, active: i % 2 === 0 });
  }
  const n = await users.createMany(many, 1_000_000);
  assert.equal(n, 12_000);
  assert.equal(await users.count(), 12_000);

  // Heterogeneous rows fail loudly instead of inserting silent NULLs.
  await assert.rejects(
    async () =>
      users.createMany([
        { id: 20_001, name: "full", score: 1, active: true },
        { id: 20_002, name: "missing-active", score: 1 },
      ]),
    (e: unknown) => e instanceof PowderError && /row 1 has columns/.test((e as Error).message),
  );
});

test("$extend grafts user methods onto tables (and into transactions)", async () => {
  const client = await Client.connect("sqlite::memory:");
  await client.execute(
    "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, score REAL, active INTEGER NOT NULL)",
  );
  const { extendPowder } = await import("./orm.js");
  const base = {
    users: new PowderTable<User>(client, USERS_META),
    $transaction: <T>(fn: (tx: { users: PowderTable<User> }) => Promise<T>): Promise<T> =>
      client.transaction(() => fn(base)),
  };
  const db = extendPowder(base, {
    users: {
      async top(this: PowderTable<User>, n: number): Promise<User[]> {
        return this.orderBy("score", "desc").limit(n).all();
      },
      async createManyActive(this: PowderTable<User>, names: string[]): Promise<number> {
        return this.createMany(
          names.map((name, i) => ({ id: 100 + i, name, score: 0, active: true })),
        );
      },
    },
  });

  // Custom method composes built-ins; built-ins still work on the extended table.
  await db.users.createManyActive(["a", "b", "c"]);
  assert.equal(await db.users.count(), 3);
  await db.users.update({ where: { name: "a" }, data: { score: 9 } });
  const top = await db.users.top(1);
  assert.equal(top[0]?.name, "a");

  // Extensions survive into transactions; unknown table name fails loudly.
  await db.$transaction(async (tx) => {
    const t = await tx.users.top(2);
    assert.equal(t.length, 2);
  });
  assert.throws(
    () => extendPowder(base, { nope: { x() {} } } as never),
    PowderError,
  );
});

// --- OR / nested logical WHERE conditions -----------------------------------

async function seedUsers(rows: User[]): Promise<PowderTable<User>> {
  const users = await setup();
  await users.createMany(rows);
  return users;
}

test("where OR: two branches", async () => {
  const users = await seedUsers([
    { id: 1, name: "alice", score: 95, active: true },
    { id: 2, name: "bob", score: 40, active: true },
    { id: 3, name: "vip_carol", score: 10, active: true },
  ]);

  const rows = await users.findMany({
    where: { OR: [{ score: { gte: 90 } }, { name: { like: "vip%" } }] },
    orderBy: { id: "asc" },
  });
  assert.deepEqual(rows.map((r) => r.id), [1, 3]);
});

test("where NOT and nested AND inside OR + top-level AND precedence", async () => {
  const users = await seedUsers([
    { id: 1, name: "a", score: 95, active: true },
    { id: 2, name: "b", score: 95, active: false },
    { id: 3, name: "c", score: 5, active: true },
  ]);

  // active = true AND (score >= 90 OR (id = 3 AND score = 5))
  const rows = await users.findMany({
    where: {
      active: true,
      OR: [{ score: { gte: 90 } }, { AND: [{ id: 3 }, { score: 5 }] }],
    },
    orderBy: { id: "asc" },
  });
  assert.deepEqual(rows.map((r) => r.id), [1, 3]);

  const notRows = await users.findMany({
    where: { NOT: { active: true } },
    orderBy: { id: "asc" },
  });
  assert.deepEqual(notRows.map((r) => r.id), [2]);
});

test("where empty OR matches nothing; delete of effectively-empty where is rejected", async () => {
  const users = await seedUsers([{ id: 1, name: "a", score: 1, active: true }]);

  assert.equal((await users.findMany({ where: { OR: [] } })).length, 0);
  await assert.rejects(() => users.delete({ AND: [] }), /non-empty where/);
  assert.equal(await users.count(), 1); // table untouched
});

test("where param order stays in sync across mixed ops + logical groups", async () => {
  const users = await seedUsers([
    { id: 1, name: "alice", score: 50, active: true },
    { id: 2, name: "alan", score: 80, active: true },
    { id: 3, name: "zoe", score: 80, active: true },
  ]);

  // name LIKE 'al%' AND (score = 80 OR id IN (1))
  const rows = await users.findMany({
    where: { name: { like: "al%" }, OR: [{ score: 80 }, { id: { in: [1] } }] },
    orderBy: { id: "asc" },
  });
  assert.deepEqual(rows.map((r) => r.id), [1, 2]);
});

test("unknown column inside OR throws PowderError", async () => {
  const users = await setup();
  await assert.rejects(
    () => users.findMany({ where: { OR: [{ nope: 1 } as unknown as Where<User>] } }),
    /unknown column `nope`/,
  );
});

test("finder: chained where() combines a column filter with an OR group across calls", async () => {
  const users = await seedUsers([
    { id: 1, name: "alice", score: 95, active: true },
    { id: 2, name: "alan", score: 10, active: true },
    { id: 3, name: "zed", score: 95, active: true },
  ]);

  // (name LIKE 'al%') AND (score >= 90 OR id = 2)  -> ids 1, 2
  const rows = await users
    .where({ name: { like: "al%" } })
    .where({ OR: [{ score: { gte: 90 } }, { id: 2 }] })
    .orderBy("id")
    .all();
  assert.deepEqual(rows.map((r) => r.id), [1, 2]);
});
