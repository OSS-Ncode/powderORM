/** Fluent, injection-safe SQL builder — the idiomatic Node-side mirror of the
 * Rust `Query` builder. Produces `{ sql, params }` for `Client.query`. */

export type Param = number | bigint | string | boolean | null;

export class Query {
  private cols: string[] = [];
  private wheres: string[] = [];
  private params: Param[] = [];
  private orderBy?: string;
  private limitN?: number;
  private offsetN?: number;

  private constructor(private readonly tableName: string) {}

  static table(name: string): Query {
    return new Query(name);
  }

  select(...columns: string[]): this {
    this.cols = columns;
    return this;
  }

  /** Add a `WHERE` predicate; one `?` per supplied param. ANDed together. */
  filter(predicate: string, ...params: Param[]): this {
    this.wheres.push(predicate);
    this.params.push(...params);
    return this;
  }

  order(column: string, direction: "ASC" | "DESC" = "ASC"): this {
    this.orderBy = `${column} ${direction}`;
    return this;
  }

  limit(n: number): this {
    this.limitN = n;
    return this;
  }

  offset(n: number): this {
    this.offsetN = n;
    return this;
  }

  build(): { sql: string; params: Param[] } {
    const cols = this.cols.length ? this.cols.join(", ") : "*";
    let sql = `SELECT ${cols} FROM ${this.tableName}`;
    if (this.wheres.length) sql += ` WHERE ${this.wheres.join(" AND ")}`;
    if (this.orderBy) sql += ` ORDER BY ${this.orderBy}`;
    if (this.limitN !== undefined) sql += ` LIMIT ${this.limitN}`;
    if (this.offsetN !== undefined) sql += ` OFFSET ${this.offsetN}`;
    return { sql, params: this.params };
  }
}
