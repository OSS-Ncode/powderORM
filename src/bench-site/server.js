// Ncode performance-test website.
//
// Compares how fast a SQLite result set becomes usable JS values, across:
//   1. Ncode zero-copy    — read straight off the NCB typed-array views.
//   2. Ncode toRows()     — same NCB buffer, but materialized into one JS object per row.
//   3. node:sqlite .all() — Node's built-in driver: one JS object per row, no columnar step.
//   4. Drizzle ORM        — .select()...orderBy() over better-sqlite3.
//   5. Prisma ORM         — .findMany() via the better-sqlite3 driver adapter.
//
// (1) and (2) hit the exact same query result; the only difference is whether
// the JS side pays the per-row object-allocation cost. (3)-(5) are the row-object
// path every ORM / driver effectively takes.

import http from "node:http";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { dirname, join, extname } from "node:path";
import { DatabaseSync } from "node:sqlite";
import BetterSqlite3 from "better-sqlite3";
import { drizzle } from "drizzle-orm/better-sqlite3";
import { asc } from "drizzle-orm";
import { PrismaBetterSqlite3 } from "@prisma/adapter-better-sqlite3";
import { Client, Query } from "../crates/ncode-node/dist/index.js";
import { PrismaClient } from "./generated/prisma/client.ts";
import { benchUsers } from "./drizzle-schema.js";

const __dirname = dirname(fileURLToPath(import.meta.url));
const PUBLIC_DIR = join(__dirname, "public");
const PORT = process.env.PORT ? Number(process.env.PORT) : 4173;
const CHUNK_ROWS = 500; // rows per bulk-insert statement (keeps SQL param count sane)

function generateData(n) {
  const ids = new Array(n);
  const names = new Array(n);
  const scores = new Array(n);
  for (let i = 0; i < n; i++) {
    ids[i] = i + 1;
    names[i] = `user_${i}_${(i * 2654435761) % 1000}`;
    scores[i] = Math.round(((i * 37) % 10000) / 7) / 10;
  }
  return { ids, names, scores };
}

function* chunks(n, size) {
  for (let start = 0; start < n; start += size) {
    yield [start, Math.min(start + size, n)];
  }
}

async function seedNcode(db, data) {
  await db.execute("CREATE TABLE bench_users (id INTEGER, name TEXT, score REAL)");
  for (const [start, end] of chunks(data.ids.length, CHUNK_ROWS)) {
    const placeholders = [];
    const params = [];
    for (let i = start; i < end; i++) {
      placeholders.push("(?, ?, ?)");
      params.push(data.ids[i], data.names[i], data.scores[i]);
    }
    await db.execute(
      `INSERT INTO bench_users (id, name, score) VALUES ${placeholders.join(", ")}`,
      params,
    );
  }
}

function seedBaseline(db, data) {
  db.exec("CREATE TABLE bench_users (id INTEGER, name TEXT, score REAL)");
  for (const [start, end] of chunks(data.ids.length, CHUNK_ROWS)) {
    const placeholders = [];
    const params = [];
    for (let i = start; i < end; i++) {
      placeholders.push("(?, ?, ?)");
      params.push(data.ids[i], data.names[i], data.scores[i]);
    }
    db.prepare(`INSERT INTO bench_users (id, name, score) VALUES ${placeholders.join(", ")}`).run(
      ...params,
    );
  }
}

// Force full materialization of a NCB batch without allocating per-row
// objects: this is the shape of access Ncode's zero-copy format is built for.
function zeroCopySum(batch) {
  const ids = batch.column("id");
  const names = batch.column("name");
  const scores = batch.column("score");
  let sum = 0;
  let nameLen = 0;
  for (let i = 0; i < batch.numRows; i++) {
    sum += Number(scores.get(i) ?? 0);
    nameLen += names.get(i)?.length ?? 0;
    sum += Number(ids.get(i)); // touch every column
  }
  return sum + nameLen;
}

async function seedPrisma(prisma, data) {
  await prisma.$executeRawUnsafe(
    "CREATE TABLE bench_users (id INTEGER PRIMARY KEY, name TEXT, score REAL)",
  );
  for (const [start, end] of chunks(data.ids.length, CHUNK_ROWS)) {
    const rows = [];
    for (let i = start; i < end; i++) {
      rows.push({ id: data.ids[i], name: data.names[i], score: data.scores[i] });
    }
    await prisma.benchUser.createMany({ data: rows });
  }
}

function objectRowsSum(rows) {
  let sum = 0;
  let nameLen = 0;
  for (const row of rows) {
    sum += Number(row.score ?? 0);
    nameLen += row.name?.length ?? 0;
    sum += Number(row.id);
  }
  return sum + nameLen;
}

function median(values) {
  const sorted = [...values].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
}

async function runTrial(data) {
  // --- Ncode path -----------------------------------------------------
  const ncodeDb = await Client.connect("sqlite::memory:");
  await seedNcode(ncodeDb, data);

  let t0 = performance.now();
  const batch = await ncodeDb.run(
    Query.table("bench_users").select("id", "name", "score").order("id", "ASC"),
  );
  const ncodeQueryMs = performance.now() - t0;

  t0 = performance.now();
  const zeroCopyChecksum = zeroCopySum(batch);
  const ncodeZeroCopyMs = performance.now() - t0;

  t0 = performance.now();
  const rowsFromNcode = batch.toRows();
  const ncodeToRowsMs = performance.now() - t0;
  const ncodeToRowsChecksum = objectRowsSum(rowsFromNcode);

  // --- baseline: node:sqlite (traditional row-object driver) ----------
  const baselineDb = new DatabaseSync(":memory:");
  seedBaseline(baselineDb, data);

  t0 = performance.now();
  const baselineRows = baselineDb
    .prepare("SELECT id, name, score FROM bench_users ORDER BY id ASC")
    .all();
  const baselineAllMs = performance.now() - t0;
  const baselineChecksum = objectRowsSum(baselineRows);

  baselineDb.close();

  // --- Drizzle ORM (over better-sqlite3) -------------------------------
  const drizzleSqlite = new BetterSqlite3(":memory:");
  seedBaseline(drizzleSqlite, data); // same raw-SQL shape node:sqlite used
  const drizzleDb = drizzle(drizzleSqlite);

  t0 = performance.now();
  const drizzleRows = await drizzleDb
    .select()
    .from(benchUsers)
    .orderBy(asc(benchUsers.id));
  const drizzleAllMs = performance.now() - t0;
  const drizzleChecksum = objectRowsSum(drizzleRows);

  drizzleSqlite.close();

  // --- Prisma ORM (better-sqlite3 driver adapter) ----------------------
  const prismaAdapter = new PrismaBetterSqlite3({ url: ":memory:" });
  const prisma = new PrismaClient({ adapter: prismaAdapter });
  await seedPrisma(prisma, data);

  t0 = performance.now();
  const prismaRows = await prisma.benchUser.findMany({ orderBy: { id: "asc" } });
  const prismaAllMs = performance.now() - t0;
  const prismaChecksum = objectRowsSum(prismaRows);

  await prisma.$disconnect();

  return {
    ncodeQueryMs,
    ncodeZeroCopyMs,
    ncodeToRowsMs,
    baselineAllMs,
    drizzleAllMs,
    prismaAllMs,
    checksumsMatch:
      Math.abs(zeroCopyChecksum - ncodeToRowsChecksum) < 1e-6 &&
      Math.abs(zeroCopyChecksum - baselineChecksum) < 1e-6 &&
      Math.abs(zeroCopyChecksum - drizzleChecksum) < 1e-6 &&
      Math.abs(zeroCopyChecksum - prismaChecksum) < 1e-6,
  };
}

async function runBenchmark(rows, trials) {
  const data = generateData(rows);
  const results = [];
  for (let i = 0; i < trials; i++) {
    results.push(await runTrial(data));
  }

  const pick = (key) => results.map((r) => r[key]);
  const summarize = (key) => ({
    medianMs: Number(median(pick(key)).toFixed(3)),
    minMs: Number(Math.min(...pick(key)).toFixed(3)),
    maxMs: Number(Math.max(...pick(key)).toFixed(3)),
  });

  return {
    rows,
    trials,
    verified: results.every((r) => r.checksumsMatch),
    metrics: {
      ncodeRawAccess: summarize("ncodeZeroCopyMs"), // decode-to-typed-array access time only
      ncodeQuery: summarize("ncodeQueryMs"), // native call + NCB decode
      ncodeToRows: summarize("ncodeToRowsMs"), // materializing JS objects from the batch
      baselineAll: summarize("baselineAllMs"), // node:sqlite driver, JS objects directly
      drizzleAll: summarize("drizzleAllMs"), // drizzle-orm over better-sqlite3
      prismaAll: summarize("prismaAllMs"), // prisma ORM, better-sqlite3 driver adapter
    },
  };
}

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
};

const server = http.createServer(async (req, res) => {
  try {
    const url = new URL(req.url, `http://${req.headers.host}`);

    if (url.pathname === "/api/bench") {
      const rows = Math.max(1, Math.min(200_000, Number(url.searchParams.get("rows") ?? 20_000)));
      const trials = Math.max(1, Math.min(20, Number(url.searchParams.get("trials") ?? 5)));
      const result = await runBenchmark(rows, trials);
      res.writeHead(200, { "content-type": "application/json; charset=utf-8" });
      res.end(JSON.stringify(result));
      return;
    }

    let filePath = url.pathname === "/" ? "/index.html" : url.pathname;
    filePath = join(PUBLIC_DIR, filePath);
    if (!filePath.startsWith(PUBLIC_DIR)) {
      res.writeHead(403);
      res.end("forbidden");
      return;
    }
    const body = await readFile(filePath);
    res.writeHead(200, { "content-type": MIME[extname(filePath)] ?? "application/octet-stream" });
    res.end(body);
  } catch (err) {
    res.writeHead(err.code === "ENOENT" ? 404 : 500, { "content-type": "application/json" });
    res.end(JSON.stringify({ error: String(err.message ?? err) }));
  }
});

server.listen(PORT, () => {
  console.log(`Ncode benchmark site: http://localhost:${PORT}`);
});
