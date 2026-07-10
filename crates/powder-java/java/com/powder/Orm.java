package com.powder;

import java.util.List;
import java.util.Map;

/**
 * The model layer over a {@link Client}: the same operation semantics as the
 * TS/Python/Go/C# ORMs, executed by the shared Rust engine. Options are maps
 * with the unified keys — {@code where}, {@code orderBy}, {@code limit},
 * {@code offset}, {@code include}, {@code join}. Rows come back as
 * {@code List<Map<String, Object>>} (insertion-ordered maps).
 *
 * <pre>
 * try (Orm orm = db.orm(schemaJson)) {              // powder.schema.json text
 *     Orm.Table users = orm.table("users");
 *     users.create(Map.of("id", 1, "name", "alice", "score", 9.5, "active", true));
 *     List&lt;Map&lt;String, Object&gt;&gt; rows = users.findMany(Map.of(
 *         "where", Map.of("active", true, "score", Map.of("gte", 5)),
 *         "orderBy", Map.of("score", "desc"),
 *         "limit", 10));
 * }
 * </pre>
 *
 * Note: {@link Map#of} does not preserve key order — for a multi-column
 * {@code orderBy}, pass a {@link java.util.LinkedHashMap}.
 */
public final class Orm implements AutoCloseable {
    private final Client client;
    private long schema;

    Orm(Client client, String schemaJson) {
        this.client = client;
        this.schema = PowderNative.ormSchemaNew(schemaJson);
    }

    /** Handle for one table's CRUD surface. */
    public Table table(String name) {
        return new Table(name);
    }

    @Override
    public void close() {
        if (schema != 0) {
            PowderNative.ormSchemaFree(schema);
            schema = 0;
        }
    }

    private long schemaHandle() {
        if (schema == 0) {
            throw new IllegalStateException("orm is closed");
        }
        return schema;
    }

    /** One table's unified CRUD surface. */
    public final class Table {
        private final String name;

        private Table(String name) {
            this.name = name;
        }

        /** Rows matching {@code opts}; null opts = all rows. */
        @SuppressWarnings("unchecked")
        public List<Map<String, Object>> findMany(Map<String, ?> opts) {
            return (List<Map<String, Object>>) find("findMany", opts);
        }

        /** First matching row, or null. */
        @SuppressWarnings("unchecked")
        public Map<String, Object> findFirst(Map<String, ?> opts) {
            return (Map<String, Object>) find("findFirst", opts);
        }

        /** Every row. */
        public List<Map<String, Object>> all() {
            return findMany(null);
        }

        /** INSERT one row; missing (nullable) columns are omitted. */
        public long create(Map<String, ?> data) {
            return execute("create", Map.of("data", data));
        }

        /** Bulk INSERT (chunked multi-row VALUES); every row must carry the
         *  same columns as the first. */
        public long createMany(List<? extends Map<String, ?>> rows) {
            return execute("createMany", Map.of("rows", rows));
        }

        /** UPDATE matching rows; returns the affected count. */
        public long update(Map<String, ?> where, Map<String, ?> data) {
            return execute("update", Map.of("where", where, "data", data));
        }

        /** DELETE matching rows. An empty where is rejected — use
         *  {@link #deleteAll}. */
        public long delete(Map<String, ?> where) {
            return execute("delete", Map.of("where", where));
        }

        /** DELETE every row (explicit opt-in). */
        public long deleteAll() {
            return execute("deleteAll", null);
        }

        /** COUNT rows matching where (null counts everything). */
        public long count(Map<String, ?> where) {
            return execute("count", where == null ? null : Map.of("where", where));
        }

        /** Whether at least one row matches. */
        public boolean exists(Map<String, ?> where) {
            return findFirst(where == null ? Map.of("limit", 1) : Map.of("where", where, "limit", 1)) != null;
        }

        /** SUM/AVG/MIN/MAX over one column; null when no rows match. */
        public Double aggregate(String fn, String column, Map<String, ?> where) {
            Object v = find("aggregate", where == null
                    ? Map.of("fn", fn, "column", column)
                    : Map.of("fn", fn, "column", column, "where", where));
            return v == null ? null : ((Number) v).doubleValue();
        }

        /** GROUP BY with aggregates ({@code by}, {@code count}, {@code sum},
         *  {@code avg}, {@code min}, {@code max}, {@code having},
         *  {@code orderBy}, ...); aggregates come back aliased {@code _count},
         *  {@code _sum_&lt;col&gt;}, .... */
        @SuppressWarnings("unchecked")
        public List<Map<String, Object>> groupBy(Map<String, ?> opts) {
            return (List<Map<String, Object>>) find("groupBy", opts);
        }

        private long execute(String op, Map<String, ?> opts) {
            return PowderNative.ormExecute(client.handle(), schemaHandle(), opJson(op, opts));
        }

        private Object find(String op, Map<String, ?> opts) {
            String json = PowderNative.ormFindJson(client.handle(), schemaHandle(), opJson(op, opts));
            return Json.read(json);
        }

        private String opJson(String op, Map<String, ?> opts) {
            java.util.LinkedHashMap<String, Object> node = new java.util.LinkedHashMap<>();
            node.put("op", op);
            node.put("table", name);
            if (opts != null) {
                node.putAll(opts);
            }
            return Json.write(node);
        }
    }
}
