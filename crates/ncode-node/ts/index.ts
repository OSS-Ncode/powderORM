/**
 * Ncode — Node.js client.
 *
 * A thin, idiomatic layer over the native napi-rs addon: async `connect`, a
 * `Client` whose `query` resolves to a decoded zero-copy columnar batch, and
 * the fluent {@link Query} builder.
 */
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { readdirSync } from "node:fs";
import { decodeBatch, type NcodeBatch } from "./reader.js";
import { Query, type Param } from "./query.js";

export { Query, decodeBatch };
export type { Param, NcodeBatch };
export { DataType } from "./reader.js";
export type { NcodeColumn, Scalar } from "./reader.js";

// The compiled addon (`napi build` emits `ncode-node.<platform>.node`). A
// generated `index.node` loader is conventional; fall back to a direct require.
interface NativeClient {
  execute(sql: string, params?: Param[]): Promise<number>;
  query(sql: string, params?: Param[]): Promise<Buffer>;
}
interface NativeModule {
  connect(url: string): Promise<NativeClient>;
}

const require = createRequire(import.meta.url);
// The napi platform binary (`index.<triple>.node`) sits at the package root.
// Loading the `.node` directly sidesteps the CJS/ESM loader mismatch.
const pkgRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const binary = readdirSync(pkgRoot).find(
  (f) => f.startsWith("index.") && f.endsWith(".node"),
);
if (!binary) {
  throw new Error(`no Ncode native addon (index.*.node) found in ${pkgRoot}`);
}
const native: NativeModule = require(join(pkgRoot, binary));

/** An async database client backed by the Rust core. */
export class Client {
  private constructor(private readonly inner: NativeClient) {}

  /** Open a connection (e.g. `"sqlite::memory:"` or a file path). */
  static async connect(url: string): Promise<Client> {
    return new Client(await native.connect(url));
  }

  /** Run a non-row statement (INSERT/UPDATE/DDL); resolves to rows affected. */
  execute(sql: string, params: Param[] = []): Promise<number> {
    return this.inner.execute(sql, params);
  }

  /** Run a query; resolves to a decoded, zero-copy columnar {@link NcodeBatch}. */
  async query(sql: string, params: Param[] = []): Promise<NcodeBatch> {
    const bytes = await this.inner.query(sql, params);
    return decodeBatch(bytes);
  }

  /** Run a built {@link Query}. */
  async run(query: Query): Promise<NcodeBatch> {
    const { sql, params } = query.build();
    return this.query(sql, params);
  }
}
