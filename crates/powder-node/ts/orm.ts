/**
 * Powder ORM — the model layer over the Powder driver.
 *
 * A {@link PowderTable} is constructed from AOT-compiled metadata (emitted by
 * `powder generate` from `powder.schema.json`): the SQL skeletons for every
 * base operation are precompiled strings, and column identifiers are resolved
 * and quoted at generation time. At runtime no query is parsed or re-planned —
 * fragments are concatenated and parameters bound.
 *
 * Errors are wrapped in {@link PowderError}, which carries the failing SQL and
 * the *caller's* source location (`file:line:col`) so terminals render a
 * clickable "warp" link straight to the offending TS line.
 */

import type { Client } from "./index.js";
import type { PowderBatch, Scalar } from "./reader.js";

/** Logical column types shared with `powder.schema.json`. */
export type ColumnType = "int" | "float" | "text" | "bool";

export interface ColumnMeta {
  readonly name: string;
  readonly type: ColumnType;
  readonly nullable?: boolean;
  readonly primaryKey?: boolean;
}

/**
 * A relation derived from a foreign key.
 * - `belongsTo`: this table's FK points at `target` — attaches one row (or null).
 * - `hasMany`: `target`'s FK points back here — attaches an array of rows.
 *
 * Columns are ordered arrays so composite (multi-column) keys join correctly:
 * row `r` matches target row `t` iff `r[localColumns[i]] === t[foreignColumns[i]]`
 * for every `i`.
 */
export interface RelationMeta {
  /** Property name the related row(s) are attached under (e.g. `user`, `posts`). */
  readonly name: string;
  readonly kind: "belongsTo" | "hasMany";
  readonly localColumns: readonly string[];
  readonly foreignColumns: readonly string[];
  /** Thunk so mutually-referencing tables can be declared in any order. */
  readonly target: () => TableMeta;
}

/** AOT-compiled table metadata (normally emitted by `powder generate`). */
export interface TableMeta {
  readonly table: string;
  readonly columns: readonly ColumnMeta[];
  /** Precompiled SQL skeletons; runtime only appends bound predicates. */
  readonly sql: {
    readonly selectAll: string;
    readonly insert: string;
    readonly countAll: string;
    readonly deleteAll: string;
    /** column name -> quoted identifier, resolved at generation time. */
    readonly ident: Readonly<Record<string, string>>;
  };
  /** Foreign-key relations loadable via `include`. */
  readonly relations?: readonly RelationMeta[];
}

/** Comparison operators accepted inside a where clause. */
export interface WhereOps<V> {
  eq?: V | null;
  ne?: V | null;
  gt?: V;
  gte?: V;
  lt?: V;
  lte?: V;
  like?: string;
  in?: readonly V[];
}

/** Logical grouping keys, Prisma-style. Combine with column conditions (all AND'd). */
export type WhereLogic<T> = {
  AND?: Where<T> | Where<T>[];
  OR?: Where<T>[];
  NOT?: Where<T> | Where<T>[];
};

export type Where<T> = {
  [K in keyof T]?: T[K] | WhereOps<NonNullable<T[K]>> | null;
} & WhereLogic<T>;

/**
 * Relation include map. A value of `true` loads that relation; an object form
 * nests further: `{ user: { include: { posts: true } } }` loads each post's
 * user and then every user's posts, one batched query per relation level.
 */
export type IncludeMap = Record<string, boolean | { include?: IncludeMap }>;

export interface FindOptions<T, Inc extends IncludeMap = IncludeMap> {
  where?: Where<T>;
  orderBy?: { [K in keyof T]?: "asc" | "desc" };
  limit?: number;
  offset?: number;
  /**
   * Relations to load with a second `IN` query and attach (no N+1). Works for
   * both `belongsTo` and `hasMany` relations, composite keys, and nesting
   * (see {@link IncludeMap}).
   */
  include?: Inc;
  /**
   * `belongsTo` relations to hydrate in a *single* `LEFT JOIN` query instead
   * of a second round-trip. Only valid for `belongsTo`; `hasMany` would
   * multiply parent rows, so use `include` for those.
   */
  join?: { [K in keyof Inc]?: boolean };
}

/** A database error mapped back to the caller's source location. */
export class PowderError extends Error {
  /** The SQL that failed. */
  readonly sql: string;
  /** `file:line:col` of the application call site, when resolvable. */
  readonly site?: string;

  constructor(message: string, sql: string, site?: string) {
    super(site ? `${message}\n  query: ${sql}\n  at ${site}` : `${message}\n  query: ${sql}`);
    this.name = "PowderError";
    this.sql = sql;
    this.site = site;
  }
}

const OPS: Record<keyof WhereOps<unknown>, string> = {
  eq: "=",
  ne: "<>",
  gt: ">",
  gte: ">=",
  lt: "<",
  lte: "<=",
  like: "LIKE",
  in: "IN",
};

type Param = number | bigint | string | boolean | null;

/** Capture the first stack frame that lives outside this package / node core. */
function callSite(): string | undefined {
  const stack = new Error().stack;
  if (!stack) return undefined;
  for (const line of stack.split("\n").slice(1)) {
    // Skip this module's own frames and node internals; the first remaining
    // frame is the application call site.
    if (line.includes("node:internal") || /[/\\](dist|ts)[/\\]orm\.(js|ts)/.test(line)) {
      continue;
    }
    const m = line.match(/\(?((?:[A-Za-z]:)?[^():]+?):(\d+):(\d+)\)?\s*$/);
    if (m) return `${m[1].replace(/^file:\/\/\/?/, "")}:${m[2]}:${m[3]}`;
  }
  return undefined;
}

/** Coerce a raw PCB scalar to the model's declared column type. */
function coerce(v: Scalar, type: ColumnType): unknown {
  if (v === null) return null;
  if (type === "bool") return v === true || v === 1 || v === 1n;
  if (typeof v === "bigint") {
    return v >= BigInt(Number.MIN_SAFE_INTEGER) && v <= BigInt(Number.MAX_SAFE_INTEGER)
      ? Number(v)
      : v;
  }
  return v;
}

function toParam(v: unknown): Param {
  if (v === undefined) return null;
  return v as Param;
}

/**
 * Cached per-meta row factories: a generated function builds each row as one
 * monomorphic object literal (stable hidden class) instead of per-property
 * dynamic assignment. Falls back to the dynamic loop under a CSP that forbids
 * `new Function`. The factory depends only on the column names, so it is
 * compiled once per {@link TableMeta} and reused across queries.
 */
type RowMaker = (...getters: Array<(r: number) => unknown>) => (r: number) => Record<string, unknown>;
const rowMakerCache = new WeakMap<TableMeta, RowMaker | null>();

function rowMakerFor(meta: TableMeta): RowMaker | null {
  let make = rowMakerCache.get(meta);
  if (make === undefined) {
    try {
      const args = meta.columns.map((_, i) => `g${i}`);
      const body = `return (r) => ({${meta.columns
        .map((c, i) => `${JSON.stringify(c.name)}: g${i}(r)`)
        .join(", ")}});`;
      make = new Function(...args, body) as RowMaker;
    } catch {
      make = null; // CSP forbids codegen — use the dynamic path.
    }
    rowMakerCache.set(meta, make);
  }
  return make;
}

/** Materialize a batch into plain objects following a table's column meta. */
function materialize(batch: PowderBatch, meta: TableMeta): Record<string, unknown>[] {
  const out: Record<string, unknown>[] = new Array(batch.numRows);
  const make = rowMakerFor(meta);
  if (make) {
    const getters = meta.columns.map((c) => {
      const col = batch.column(c.name);
      const t = c.type;
      return col ? (r: number) => coerce(col.get(r), t) : () => null;
    });
    const rowOf = make(...getters);
    for (let r = 0; r < batch.numRows; r++) out[r] = rowOf(r);
    return out;
  }
  const cols = meta.columns.map((c) => ({ meta: c, col: batch.column(c.name) }));
  for (let r = 0; r < batch.numRows; r++) {
    const row: Record<string, unknown> = {};
    for (const { meta: cm, col } of cols) {
      row[cm.name] = col ? coerce(col.get(r), cm.type) : null;
    }
    out[r] = row;
  }
  return out;
}

/**
 * A typed table handle: the unified CRUD surface of Powder ORM.
 *
 * @example
 * const users = new PowderTable<User>(client, USERS_META);
 * await users.create({ id: 1, name: "alice", score: 9.5 });
 * const top = await users.findMany({ where: { score: { gte: 5 } }, orderBy: { id: "asc" } });
 */
export class PowderTable<T extends object, Inc extends IncludeMap = IncludeMap> {
  /**
   * Compiled WHERE fragments, keyed by the *shape* of the predicate (columns,
   * operators, and `IN` arity — never the bound values). Two calls with the
   * same shape reuse the string instead of rebuilding and joining it; only the
   * parameters are re-collected. Bounded so a pathological caller cannot grow
   * it without limit.
   */
  private readonly whereCache = new Map<string, string>();
  private static readonly WHERE_CACHE_MAX = 256;

  constructor(
    private readonly client: Client,
    readonly meta: TableMeta,
  ) {}

  /** Structural key for a predicate: identical shapes produce identical SQL. */
  private whereShape(where: Where<T>, qualify: string): string {
    let key = qualify;
    for (const [col, cond] of Object.entries(where)) {
      if (col === "AND" || col === "OR" || col === "NOT") continue;
      if (cond === undefined) continue;
      if (cond === null) {
        key += `${col}null`;
      } else if (typeof cond === "object" && !Array.isArray(cond)) {
        key += `${col}{`;
        for (const [op, value] of Object.entries(cond as WhereOps<unknown>)) {
          if (value === undefined || !OPS[op as keyof WhereOps<unknown>]) continue;
          key +=
            op === "in"
              ? `in:${(value as unknown[]).length},`
              : value === null
                ? `${op}:null,`
                : `${op},`;
        }
        key += "}";
      } else {
        key += `${col}=`;
      }
    }
    const logic = where as WhereLogic<T>;
    if (logic.AND !== undefined) {
      key += "AND[";
      for (const w of Array.isArray(logic.AND) ? logic.AND : [logic.AND]) key += this.whereShape(w, qualify) + ";";
      key += "]";
    }
    if (logic.OR !== undefined) {
      key += `OR${logic.OR.length}[`;
      for (const w of logic.OR) key += this.whereShape(w, qualify) + ";";
      key += "]";
    }
    if (logic.NOT !== undefined) {
      key += "NOT[";
      for (const w of Array.isArray(logic.NOT) ? logic.NOT : [logic.NOT]) key += this.whereShape(w, qualify) + ";";
      key += "]";
    }
    return key;
  }

  /** Collect bound values in placeholder order, mirroring {@link renderGroup}. */
  private collectWhereParams(where: Where<T>): Param[] {
    const params: Param[] = [];
    for (const [col, cond] of Object.entries(where)) {
      if (col === "AND" || col === "OR" || col === "NOT") continue;
      if (cond === undefined || cond === null) continue;
      if (typeof cond === "object" && !Array.isArray(cond)) {
        for (const [op, value] of Object.entries(cond as WhereOps<unknown>)) {
          if (value === undefined || !OPS[op as keyof WhereOps<unknown>]) continue;
          if (op === "in") {
            for (const v of value as unknown[]) params.push(toParam(v));
          } else if (value === null) {
            // `eq: null` / `ne: null` render as IS [NOT] NULL — no parameter.
          } else {
            params.push(toParam(value));
          }
        }
      } else {
        params.push(toParam(cond));
      }
    }
    const logic = where as WhereLogic<T>;
    if (logic.AND !== undefined) {
      for (const w of Array.isArray(logic.AND) ? logic.AND : [logic.AND]) {
        params.push(...this.collectWhereParams(w));
      }
    }
    if (logic.OR !== undefined) {
      for (const w of logic.OR) params.push(...this.collectWhereParams(w));
    }
    if (logic.NOT !== undefined) {
      for (const w of Array.isArray(logic.NOT) ? logic.NOT : [logic.NOT]) {
        params.push(...this.collectWhereParams(w));
      }
    }
    return params;
  }

  /** Render a where group to a SQL fragment (no leading WHERE, no outer parens).
   *  Every sub-group is parenthesized so precedence is preserved. Returns "" when
   *  the group carries no effective predicate. */
  private renderGroup(where: Where<T>, q: string): string {
    const parts: string[] = [];
    for (const [col, cond] of Object.entries(where)) {
      if (col === "AND" || col === "OR" || col === "NOT") continue;
      const bare = this.meta.sql.ident[col];
      if (!bare) throw new PowderError(`unknown column \`${col}\``, this.meta.table, callSite());
      const ident = `${q}${bare}`;
      if (cond === undefined) continue;
      if (cond === null) {
        parts.push(`${ident} IS NULL`);
      } else if (typeof cond === "object" && !Array.isArray(cond)) {
        for (const [op, value] of Object.entries(cond as WhereOps<unknown>)) {
          const sqlOp = OPS[op as keyof WhereOps<unknown>];
          if (!sqlOp || value === undefined) continue;
          if (op === "in") {
            const list = value as unknown[];
            if (list.length === 0) {
              parts.push("1 = 0"); // IN () matches nothing
            } else {
              parts.push(`${ident} IN (${list.map(() => "?").join(", ")})`);
            }
          } else if (op === "ne" && value === null) {
            parts.push(`${ident} IS NOT NULL`);
          } else if (op === "eq" && value === null) {
            parts.push(`${ident} IS NULL`);
          } else {
            parts.push(`${ident} ${sqlOp} ?`);
          }
        }
      } else {
        parts.push(`${ident} = ?`);
      }
    }
    const logic = where as WhereLogic<T>;
    if (logic.AND !== undefined) {
      for (const w of Array.isArray(logic.AND) ? logic.AND : [logic.AND]) {
        const s = this.renderGroup(w, q);
        if (s) parts.push(`(${s})`);
      }
    }
    if (logic.OR !== undefined) {
      if (logic.OR.length === 0) {
        parts.push("1 = 0"); // OR of nothing matches nothing
      } else {
        const subs = logic.OR.map((w) => this.renderGroup(w, q)).filter((s) => s);
        if (subs.length) parts.push(`(${subs.map((s) => `(${s})`).join(" OR ")})`);
      }
    }
    if (logic.NOT !== undefined) {
      for (const w of Array.isArray(logic.NOT) ? logic.NOT : [logic.NOT]) {
        const s = this.renderGroup(w, q);
        if (s) parts.push(`NOT (${s})`);
      }
    }
    return parts.join(" AND ");
  }

  /** Render the SQL fragment for a predicate (values are not read). */
  private buildWhereClause(where: Where<T>, q: string): string {
    const frag = this.renderGroup(where, q);
    return frag ? ` WHERE ${frag}` : "";
  }

  /** Compile a where object into `(fragment, params)`; AOT idents, bound values.
   * `qualify` prefixes each identifier with a table alias (for JOIN queries).
   * The fragment is memoized by predicate shape; values never enter the key. */
  private compileWhere(
    where: Where<T> | undefined,
    qualify?: string,
  ): { clause: string; params: Param[] } {
    if (!where) return { clause: "", params: [] };
    const q = qualify ? `${qualify}.` : "";
    const key = this.whereShape(where, q);
    let clause = this.whereCache.get(key);
    if (clause === undefined) {
      clause = this.buildWhereClause(where, q);
      if (this.whereCache.size >= PowderTable.WHERE_CACHE_MAX) this.whereCache.clear();
      this.whereCache.set(key, clause);
    }
    return { clause, params: this.collectWhereParams(where) };
  }

  /** ORDER BY fragments memoized by (columns, directions, qualifier) — the
   * string assembly disappears on repeat queries, same as the where cache. */
  private readonly orderCache = new Map<string, string>();

  private compileTail(opts: FindOptions<T, Inc>, qualify?: string): string {
    let tail = "";
    const q = qualify ? `${qualify}.` : "";
    if (opts.orderBy) {
      const keys = Object.entries(opts.orderBy).filter(([, d]) => d !== undefined);
      if (keys.length) {
        const cacheKey = q + keys.map(([c, d]) => `${c}:${d}`).join(",");
        let frag = this.orderCache.get(cacheKey);
        if (frag === undefined) {
          frag = ` ORDER BY ${keys
            .map(([col, dir]) => {
              const bare = this.meta.sql.ident[col];
              if (!bare) throw new PowderError(`unknown column \`${col}\``, this.meta.table, callSite());
              return `${q}${bare} ${dir === "desc" ? "DESC" : "ASC"}`;
            })
            .join(", ")}`;
          if (this.orderCache.size >= PowderTable.WHERE_CACHE_MAX) this.orderCache.clear();
          this.orderCache.set(cacheKey, frag);
        }
        tail += frag;
      }
    }
    if (opts.limit !== undefined) tail += ` LIMIT ${Math.floor(opts.limit)}`;
    if (opts.offset !== undefined) tail += ` OFFSET ${Math.floor(opts.offset)}`;
    return tail;
  }

  private rowsOf(batch: PowderBatch): T[] {
    return materialize(batch, this.meta) as T[];
  }

  /**
   * Batch-load `include`d relations and attach them. Each relation runs one
   * `IN` query over the distinct key tuples (no N+1), then rows are matched by
   * a stringified tuple of their key columns. `belongsTo` attaches one target
   * row (or null); `hasMany` attaches an array (empty when none match).
   * Object-form entries recurse: the loaded target rows get *their* relations
   * attached the same way, one batched query per level.
   */
  private async attachRelations(
    rows: readonly object[],
    include: IncludeMap,
    meta: TableMeta,
    site: string | undefined,
  ): Promise<void> {
    for (const [name, spec] of Object.entries(include)) {
      if (!spec) continue;
      const rel = meta.relations?.find((r) => r.name === name);
      if (!rel) {
        throw new PowderError(
          `unknown relation \`${name}\` (no foreign key on ${meta.table} defines it)`,
          meta.table,
          site,
        );
      }
      const target = rel.target();

      // Distinct local key tuples (skip rows with a null in any key column).
      const tupleKey = (obj: Record<string, unknown>, cols: readonly string[]): string | null => {
        const vals: unknown[] = [];
        for (const c of cols) {
          const v = obj[c];
          if (v === null || v === undefined) return null;
          vals.push(v);
        }
        return JSON.stringify(vals);
      };
      const tuples = new Map<string, unknown[]>();
      for (const row of rows) {
        const r = row as Record<string, unknown>;
        const k = tupleKey(r, rel.localColumns);
        if (k !== null && !tuples.has(k)) tuples.set(k, rel.localColumns.map((c) => r[c]));
      }

      // Group target rows by their foreign-key tuple; keep a flat list so
      // nested includes can recurse over every loaded target row once.
      const grouped = new Map<string, Record<string, unknown>[]>();
      const flat: Record<string, unknown>[] = [];
      const fidents = rel.foreignColumns.map((c) => target.sql.ident[c] ?? c);
      const tupleList = [...tuples.values()];
      const single = rel.foreignColumns.length === 1;
      for (let start = 0; start < tupleList.length; start += 500) {
        const chunk = tupleList.slice(start, start + 500);
        let clause: string;
        const params: Param[] = [];
        if (single) {
          clause = `${fidents[0]} IN (${chunk.map(() => "?").join(", ")})`;
          for (const t of chunk) params.push(t[0] as Param);
        } else {
          // (a, b) IN ((?,?), (?,?), ...)
          const cols = fidents.join(", ");
          const rowPh = `(${fidents.map(() => "?").join(", ")})`;
          clause = `(${cols}) IN (${chunk.map(() => rowPh).join(", ")})`;
          for (const t of chunk) for (const v of t) params.push(v as Param);
        }
        const sql = `${target.sql.selectAll} WHERE ${clause}`;
        const batch = await this.runQuery(sql, params, site);
        for (const trow of materialize(batch, target)) {
          const k = tupleKey(trow, rel.foreignColumns);
          if (k === null) continue;
          flat.push(trow);
          const bucket = grouped.get(k);
          if (bucket) bucket.push(trow);
          else grouped.set(k, [trow]);
        }
      }

      // Nested include: recurse over the loaded target rows.
      if (typeof spec === "object" && spec.include && flat.length > 0) {
        await this.attachRelations(flat, spec.include, target, site);
      }

      for (const row of rows) {
        const r = row as Record<string, unknown>;
        const k = tupleKey(r, rel.localColumns);
        const matches = k === null ? [] : grouped.get(k) ?? [];
        r[rel.name] = rel.kind === "hasMany" ? matches : matches[0] ?? null;
      }
    }
  }

  /**
   * Single-query path: LEFT JOIN each requested `belongsTo` relation and
   * hydrate the nested object from aliased columns. One round-trip instead of
   * the two `include` uses.
   */
  private async findManyJoined(opts: FindOptions<T, Inc>, site: string | undefined): Promise<T[]> {
    const table = this.meta.table;
    const rels = Object.entries(opts.join ?? {})
      .filter(([, w]) => w)
      .map(([name]) => {
        const rel = this.meta.relations?.find((r) => r.name === name);
        if (!rel) {
          throw new PowderError(`unknown relation \`${name}\``, table, site);
        }
        if (rel.kind !== "belongsTo") {
          throw new PowderError(
            `relation \`${name}\` is hasMany; use include (a JOIN would multiply rows)`,
            table,
            site,
          );
        }
        return rel;
      });

    // SELECT base cols (bare names) + joined cols aliased "<rel>__<col>".
    const selects: string[] = this.meta.columns.map((c) => `${table}.${c.name} AS ${c.name}`);
    const joins: string[] = [];
    for (const rel of rels) {
      const target = rel.target();
      const alias = `j_${rel.name}`;
      for (const c of target.columns) {
        selects.push(`${alias}.${c.name} AS ${rel.name}__${c.name}`);
      }
      const on = rel.localColumns
        .map((lc, i) => `${table}.${lc} = ${alias}.${rel.foreignColumns[i]}`)
        .join(" AND ");
      joins.push(`LEFT JOIN ${target.table} AS ${alias} ON ${on}`);
    }

    const { clause, params } = this.compileWhere(opts.where, table);
    const sql =
      `SELECT ${selects.join(", ")} FROM ${table} ${joins.join(" ")}` +
      clause +
      this.compileTail(opts, table);
    const batch = await this.runQuery(sql, params, site);

    // Hydrate: base columns by name, nested belongsTo objects from aliases.
    const baseCols = this.meta.columns.map((c) => ({ meta: c, col: batch.column(c.name) }));
    const relCols = rels.map((rel) => ({
      rel,
      cols: rel.target().columns.map((c) => ({
        meta: c,
        col: batch.column(`${rel.name}__${c.name}`),
      })),
    }));
    const out: T[] = new Array(batch.numRows);
    for (let r = 0; r < batch.numRows; r++) {
      const row: Record<string, unknown> = {};
      for (const { meta, col } of baseCols) row[meta.name] = col ? coerce(col.get(r), meta.type) : null;
      for (const { rel, cols } of relCols) {
        // A LEFT JOIN miss leaves every target column null.
        let present = false;
        const nested: Record<string, unknown> = {};
        for (const { meta, col } of cols) {
          const v = col ? coerce(col.get(r), meta.type) : null;
          if (v !== null) present = true;
          nested[meta.name] = v;
        }
        row[rel.name] = present ? nested : null;
      }
      out[r] = row as T;
    }
    return out;
  }

  private async runQuery(sql: string, params: Param[], site: string | undefined): Promise<PowderBatch> {
    try {
      return await this.client.query(sql, params);
    } catch (err) {
      throw new PowderError(String((err as Error).message ?? err), sql, site);
    }
  }

  private async runExecute(sql: string, params: Param[], site: string | undefined): Promise<number> {
    try {
      return await this.client.execute(sql, params);
    } catch (err) {
      throw new PowderError(String((err as Error).message ?? err), sql, site);
    }
  }

  /** SELECT rows matching `opts`, materialized as typed objects. */
  async findMany(opts: FindOptions<T, Inc> = {}): Promise<T[]> {
    const site = callSite();
    let rows: T[];
    if (opts.join) {
      // Hydrate belongsTo relations in one LEFT JOIN query.
      rows = await this.findManyJoined(opts, site);
    } else {
      const { clause, params } = this.compileWhere(opts.where);
      const sql = this.meta.sql.selectAll + clause + this.compileTail(opts);
      rows = this.rowsOf(await this.runQuery(sql, params, site));
    }
    if (opts.include) {
      await this.attachRelations(rows as object[], opts.include, this.meta, site);
    }
    return rows;
  }

  /** First row matching `opts`, or `null`. */
  async findFirst(opts: FindOptions<T, Inc> = {}): Promise<T | null> {
    const rows = await this.findMany({ ...opts, limit: 1 });
    return rows[0] ?? null;
  }

  // -- beginner-friendly surface -----------------------------------------

  /**
   * Look up one row by primary key: `db.users.find(1)`. For composite keys
   * (or any ad-hoc lookup) pass an object: `db.grades.find({ student: 1,
   * course: "math" })`.
   */
  async find(key: number | bigint | string | Partial<T>): Promise<T | null> {
    const site = callSite();
    if (typeof key === "object" && key !== null) {
      return this.findFirst({ where: key as Where<T> });
    }
    const pk = this.meta.columns.filter((c) => c.primaryKey);
    if (pk.length !== 1) {
      throw new PowderError(
        pk.length === 0
          ? `find(value) needs a primary key on ${this.meta.table}; pass an object instead`
          : `${this.meta.table} has a composite primary key; pass an object like find({ a: ..., b: ... })`,
        this.meta.table,
        site,
      );
    }
    return this.findFirst({ where: { [pk[0].name]: key } as Where<T> });
  }

  /** Every row (optionally chain from {@link where} for filters). */
  all(): Promise<T[]> {
    return this.findMany();
  }

  /**
   * Start a chainable query. Two spellings, same engine:
   *
   * ```ts
   * db.users.where({ active: true, score: { gte: 5 } })  // object form
   * db.users.where("score", ">=", 5)                     // beginner 3-arg form
   * ```
   */
  where(w: Where<T>): Finder<T, Inc>;
  where(column: keyof T & string, op: WhereOpName, value: unknown): Finder<T, Inc>;
  where(a: Where<T> | (keyof T & string), op?: WhereOpName, value?: unknown): Finder<T, Inc> {
    const w = typeof a === "string" ? whereFromTriple<T>(a, op as WhereOpName, value) : a;
    return new Finder(this, { where: w });
  }

  /** Start a chainable query from an ordering. */
  orderBy(column: keyof T & string, dir: "asc" | "desc" = "asc"): Finder<T, Inc> {
    return new Finder(this, { orderBy: { [column]: dir } as FindOptions<T, Inc>["orderBy"] });
  }

  /** Keys of `data` that are real columns: skips `undefined` and relation
   * fields (present on rows fetched with `include`/`join`). */
  private columnKeys(data: object): string[] {
    const rels = new Set((this.meta.relations ?? []).map((r) => r.name));
    return Object.keys(data).filter(
      (k) => (data as Record<string, unknown>)[k] !== undefined && !rels.has(k),
    );
  }

  /** INSERT one row. Missing (nullable) columns are omitted. */
  async create(data: Partial<T>): Promise<number> {
    const site = callSite();
    const keys = this.columnKeys(data);
    let sql: string;
    if (keys.length === this.meta.columns.length) {
      // Full-shape insert: use the AOT statement (column order is canonical).
      sql = this.meta.sql.insert;
      const params = this.meta.columns.map((c) => toParam((data as Record<string, unknown>)[c.name]));
      return this.runExecute(sql, params, site);
    }
    const idents = keys.map((k) => {
      const ident = this.meta.sql.ident[k];
      if (!ident) throw new PowderError(`unknown column \`${k}\``, this.meta.table, site);
      return ident;
    });
    sql = `INSERT INTO ${this.meta.table} (${idents.join(", ")}) VALUES (${keys.map(() => "?").join(", ")})`;
    return this.runExecute(sql, keys.map((k) => toParam((data as Record<string, unknown>)[k])), site);
  }

  /**
   * Bulk INSERT with multi-row VALUES, chunked to keep parameter counts sane.
   *
   * Safeguards: the chunk size is clamped so a chunk never exceeds SQLite's
   * bound-variable ceiling (32766) regardless of the caller's `chunkSize`,
   * and every row must carry the same columns as the first — a row with
   * missing/extra keys fails loudly instead of silently inserting NULLs.
   */
  async createMany(rows: readonly Partial<T>[], chunkSize = 500): Promise<number> {
    if (rows.length === 0) return 0;
    const site = callSite();
    const keys = this.columnKeys(rows[0]);
    if (keys.length === 0) {
      throw new PowderError("createMany() rows have no insertable columns", this.meta.table, site);
    }
    const idents = keys.map((k) => {
      const ident = this.meta.sql.ident[k];
      if (!ident) throw new PowderError(`unknown column \`${k}\``, this.meta.table, site);
      return ident;
    });
    // SQLite's default SQLITE_MAX_VARIABLE_NUMBER is 32766; stay safely under
    // it no matter what chunk size the caller asked for.
    const MAX_VARS = 32000;
    const maxRowsPerChunk = Math.max(1, Math.floor(MAX_VARS / keys.length));
    const effectiveChunk = Math.max(1, Math.min(Math.floor(chunkSize), maxRowsPerChunk));

    const keySet = new Set<string>(keys);
    const rowPh = `(${keys.map(() => "?").join(", ")})`;
    let affected = 0;
    for (let start = 0; start < rows.length; start += effectiveChunk) {
      const chunk = rows.slice(start, start + effectiveChunk);
      const sql = `INSERT INTO ${this.meta.table} (${idents.join(", ")}) VALUES ${new Array(chunk.length).fill(rowPh).join(", ")}`;
      const params: Param[] = [];
      for (let i = 0; i < chunk.length; i++) {
        const row = chunk[i] as Record<string, unknown>;
        const rowKeys = this.columnKeys(chunk[i]);
        if (rowKeys.length !== keys.length || rowKeys.some((k) => !keySet.has(k))) {
          throw new PowderError(
            `createMany() row ${start + i} has columns [${rowKeys.join(", ")}] but row 0 has [${keys.join(", ")}]; all rows must share one shape`,
            this.meta.table,
            site,
          );
        }
        for (const k of keys) params.push(toParam(row[k]));
      }
      affected += await this.runExecute(sql, params, site);
    }
    return affected;
  }

  /** UPDATE matching rows; returns the affected count. */
  async update(args: { where: Where<T>; data: Partial<T> }): Promise<number> {
    const site = callSite();
    const rels = new Set((this.meta.relations ?? []).map((r) => r.name));
    const sets = Object.entries(args.data).filter(([k, v]) => v !== undefined && !rels.has(k));
    if (sets.length === 0) return 0;
    const setSql = sets
      .map(([k]) => {
        const ident = this.meta.sql.ident[k];
        if (!ident) throw new PowderError(`unknown column \`${k}\``, this.meta.table, site);
        return `${ident} = ?`;
      })
      .join(", ");
    const { clause, params } = this.compileWhere(args.where);
    const sql = `UPDATE ${this.meta.table} SET ${setSql}${clause}`;
    return this.runExecute(sql, [...sets.map(([, v]) => toParam(v)), ...params], site);
  }

  /** DELETE matching rows. An empty/omitted where is rejected — use {@link deleteAll}. */
  async delete(where: Where<T>): Promise<number> {
    const site = callSite();
    const { clause, params } = this.compileWhere(where);
    if (!clause) {
      throw new PowderError(
        "delete() requires a non-empty where clause; use deleteAll() to clear the table",
        this.meta.sql.deleteAll,
        site,
      );
    }
    return this.runExecute(this.meta.sql.deleteAll + clause, params, site);
  }

  /** DELETE every row (explicit opt-in). */
  async deleteAll(): Promise<number> {
    return this.runExecute(this.meta.sql.deleteAll, [], callSite());
  }

  /** COUNT rows matching `where`. */
  async count(where?: Where<T>): Promise<number> {
    const site = callSite();
    const { clause, params } = this.compileWhere(where);
    const batch = await this.runQuery(this.meta.sql.countAll + clause, params, site);
    const v = batch.column("n")?.get(0) ?? 0;
    return typeof v === "bigint" ? Number(v) : (v as number);
  }

  /** Whether at least one row matches (`LIMIT 1` under the hood). */
  async exists(where?: Where<T>): Promise<boolean> {
    return (await this.findFirst({ where } as FindOptions<T, Inc>)) !== null;
  }

  /** SUM / AVG / MIN / MAX over one column; `null` when no rows match. */
  async aggregate(
    fn: "sum" | "avg" | "min" | "max",
    column: keyof T & string,
    where?: Where<T>,
  ): Promise<number | null> {
    const site = callSite();
    const ident = this.meta.sql.ident[column];
    if (!ident) throw new PowderError(`unknown column \`${column}\``, this.meta.table, site);
    const { clause, params } = this.compileWhere(where);
    const sql = `SELECT ${fn.toUpperCase()}(${ident}) AS v FROM ${this.meta.table}${clause}`;
    const batch = await this.runQuery(sql, params, site);
    const v = batch.column("v")?.get(0);
    if (v === null || v === undefined) return null;
    return typeof v === "bigint" ? Number(v) : (v as number);
  }
}

/** Comparison spellings accepted by the 3-argument `where` overload. */
export type WhereOpName = "=" | "!=" | ">" | ">=" | "<" | "<=" | "like" | "in";

const OP_KEY: Record<WhereOpName, keyof WhereOps<unknown>> = {
  "=": "eq",
  "!=": "ne",
  ">": "gt",
  ">=": "gte",
  "<": "lt",
  "<=": "lte",
  like: "like",
  in: "in",
};

/** Turn `("score", ">=", 5)` into `{ score: { gte: 5 } }`. */
export function whereFromTriple<T>(column: keyof T & string, op: WhereOpName, value: unknown): Where<T> {
  const key = OP_KEY[op];
  if (!key) {
    throw new PowderError(
      `unknown operator \`${op}\` (expected ${Object.keys(OP_KEY).join(" ")})`,
      String(column),
      callSite(),
    );
  }
  return { [column]: { [key]: value } } as Where<T>;
}

/**
 * Execute an AOT-compiled named query (from the schema's `queries` section).
 * `sql` already uses positional `?`; `paramOrder` maps each slot to a named
 * argument. With `meta`, rows come back typed to that table's shape; without,
 * as generic column-name records. Used by `powder generate` output — call
 * through the generated `$queries` object rather than directly.
 */
export async function runNamedQuery(
  client: Client,
  sql: string,
  paramOrder: readonly string[],
  args: Record<string, unknown>,
  meta?: TableMeta,
): Promise<Record<string, unknown>[]> {
  const site = callSite();
  const params: Param[] = paramOrder.map((p) => {
    const v = args?.[p];
    if (v === undefined) {
      throw new PowderError(`missing parameter \`${p}\``, sql, site);
    }
    return toParam(v);
  });
  let batch: PowderBatch;
  try {
    batch = await client.query(sql, params);
  } catch (err) {
    throw new PowderError(String((err as Error).message ?? err), sql, site);
  }
  return meta ? materialize(batch, meta) : (batch.toRows() as Record<string, unknown>[]);
}

/**
 * A chainable, beginner-friendly query: build it up step by step and finish
 * with {@link all}, {@link first}, or {@link count}.
 *
 * ```ts
 * const top = await db.users
 *   .where({ active: true })
 *   .orderBy("score", "desc")
 *   .limit(10)
 *   .all();
 * ```
 *
 * Each step returns a new Finder, so partial queries can be shared and
 * extended without affecting each other.
 */
export class Finder<T extends object, Inc extends IncludeMap = IncludeMap> {
  constructor(
    private readonly table: PowderTable<T, Inc>,
    private readonly opts: FindOptions<T, Inc>,
  ) {}

  private extend(patch: Partial<FindOptions<T, Inc>>): Finder<T, Inc> {
    return new Finder(this.table, { ...this.opts, ...patch });
  }

  /** Add filters; multiple where() calls are merged (same column overrides).
   * Accepts both the object form and the 3-arg form: `where("score", ">=", 5)`. */
  where(w: Where<T>): Finder<T, Inc>;
  where(column: keyof T & string, op: WhereOpName, value: unknown): Finder<T, Inc>;
  where(a: Where<T> | (keyof T & string), op?: WhereOpName, value?: unknown): Finder<T, Inc> {
    const w = typeof a === "string" ? whereFromTriple<T>(a, op as WhereOpName, value) : a;
    return this.extend({ where: { ...this.opts.where, ...w } });
  }

  /** Add an ordering; multiple orderBy() calls sort by each in turn. */
  orderBy(column: keyof T & string, dir: "asc" | "desc" = "asc"): Finder<T, Inc> {
    return this.extend({
      orderBy: { ...this.opts.orderBy, [column]: dir } as FindOptions<T, Inc>["orderBy"],
    });
  }

  limit(n: number): Finder<T, Inc> {
    return this.extend({ limit: n });
  }

  offset(n: number): Finder<T, Inc> {
    return this.extend({ offset: n });
  }

  /** Load relations alongside the rows (see {@link IncludeMap}). */
  include(map: Inc): Finder<T, Inc> {
    return this.extend({ include: { ...this.opts.include, ...map } });
  }

  /** Hydrate belongsTo relations with a single LEFT JOIN query. */
  join(map: { [K in keyof Inc]?: boolean }): Finder<T, Inc> {
    return this.extend({ join: { ...this.opts.join, ...map } });
  }

  /** Run the query and return every matching row. */
  all(): Promise<T[]> {
    return this.table.findMany(this.opts);
  }

  /** Run the query and return the first row, or `null`. */
  first(): Promise<T | null> {
    return this.table.findFirst(this.opts);
  }

  /** Count matching rows (ignores limit/offset/ordering). */
  count(): Promise<number> {
    return this.table.count(this.opts.where);
  }

  /** Whether at least one row matches. */
  exists(): Promise<boolean> {
    return this.table.exists(this.opts.where);
  }

  /** One column's values, in query order: `db.users.orderBy("id").pluck("name")`. */
  async pluck<K extends keyof T & string>(column: K): Promise<T[K][]> {
    const rows = await this.all();
    return rows.map((r) => r[column]);
  }

  sum(column: keyof T & string): Promise<number | null> {
    return this.table.aggregate("sum", column, this.opts.where);
  }

  avg(column: keyof T & string): Promise<number | null> {
    return this.table.aggregate("avg", column, this.opts.where);
  }

  min(column: keyof T & string): Promise<number | null> {
    return this.table.aggregate("min", column, this.opts.where);
  }

  max(column: keyof T & string): Promise<number | null> {
    return this.table.aggregate("max", column, this.opts.where);
  }

  /**
   * Page through results: `db.users.orderBy("id").paginate(2, 20)` — 1-based
   * page number. Returns the rows plus the total (unpaged) count.
   */
  async paginate(page: number, perPage = 20): Promise<Page<T>> {
    const p = Math.max(1, Math.floor(page));
    const per = Math.max(1, Math.floor(perPage));
    const [rows, total] = await Promise.all([
      this.limit(per).offset((p - 1) * per).all(),
      this.count(),
    ]);
    return { rows, total, page: p, perPage: per, totalPages: Math.max(1, Math.ceil(total / per)) };
  }
}

/** One page of results from {@link Finder.paginate}. */
export interface Page<T> {
  rows: T[];
  total: number;
  page: number;
  perPage: number;
  totalPages: number;
}

// ---------------------------------------------------------------------------
// User-defined table methods: db.$extend({ posts: { publishAll() {...} } })
// ---------------------------------------------------------------------------

/** A bag of user methods to graft onto one table. Inside a method, `this` is
 * the table itself, so custom helpers compose the built-in API:
 *
 * ```ts
 * const xdb = db.$extend({
 *   users: {
 *     async top(this: PowderTable<Users>, n: number) {
 *       return this.orderBy("score", "desc").limit(n).all();
 *     },
 *   },
 * });
 * await xdb.users.top(3);
 * ```
 */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export type TableMethods = Record<string, (this: any, ...args: any[]) => unknown>;
export type PowderExtensions = Record<string, TableMethods>;

/** The client type after `$extend`: extended tables gain their new methods,
 * and `$transaction` hands the SAME extended client to its callback. */
export type ExtendedClient<C, E extends PowderExtensions> = {
  [K in keyof C]: K extends "$transaction"
    ? <R>(fn: (tx: ExtendedClient<C, E>) => Promise<R>) => Promise<R>
    : K extends keyof E
      ? C[K] & E[K]
      : C[K];
};

/**
 * Graft user-defined methods onto a Powder client's tables without mutating
 * the original. Unknown table names fail loudly. `$transaction` is rewrapped
 * so the same extensions are available inside transactions.
 */
export function extendPowder<C extends object, E extends PowderExtensions>(
  db: C,
  extensions: E,
): ExtendedClient<C, E> {
  const out: Record<string, unknown> = Object.create(
    Object.getPrototypeOf(db) as object | null,
  ) as Record<string, unknown>;
  Object.assign(out, db);

  for (const [tableName, methods] of Object.entries(extensions)) {
    const table = (db as Record<string, unknown>)[tableName];
    if (!(table instanceof PowderTable)) {
      throw new PowderError(
        `$extend: \`${tableName}\` is not a table on this client`,
        tableName,
        callSite(),
      );
    }
    // Prototype chain keeps the table's own methods; the extension methods
    // shadow nothing built-in unless deliberately named the same.
    const augmented = Object.create(table) as Record<string, unknown>;
    for (const [name, fn] of Object.entries(methods)) {
      augmented[name] = fn.bind(augmented);
    }
    out[tableName] = augmented;
  }

  // Transactions hand back a client with the SAME extensions applied.
  const baseTx = (db as { $transaction?: (fn: (tx: C) => Promise<unknown>) => Promise<unknown> })
    .$transaction;
  if (typeof baseTx === "function") {
    out.$transaction = <R>(fn: (tx: ExtendedClient<C, E>) => Promise<R>): Promise<R> =>
      baseTx.call(db, (tx: C) => fn(extendPowder(tx, extensions))) as Promise<R>;
  }
  return out as ExtendedClient<C, E>;
}
