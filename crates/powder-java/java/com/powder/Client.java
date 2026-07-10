package com.powder;

/**
 * An async-backed database client. Every call blocks the calling Java thread
 * while the Rust core drives the query on its own runtime.
 *
 * Bound parameters accept {@code Long}/{@code Integer}, {@code Double}/{@code
 * Float}, {@code String}, {@code Boolean}, and {@code null}.
 */
public final class Client implements AutoCloseable {
    private long handle;
    /** Transaction nesting depth: 0 = none, 1 = BEGIN, >1 = savepoints. */
    private int txDepth = 0;

    private Client(long handle) {
        this.handle = handle;
    }

    /** Open a connection (e.g. {@code "sqlite::memory:"} or a file path). */
    public static Client connect(String url) {
        return new Client(PowderNative.connect(url));
    }

    /** Run a non-row statement (INSERT/UPDATE/DDL); returns rows affected. */
    public long execute(String sql, Object... params) {
        checkOpen();
        return PowderNative.execute(handle, sql, toJsonArray(params));
    }

    /**
     * Run a query; returns a decoded columnar {@link Batch} backed by a JVM
     * {@code byte[]} (one copy at the JNI boundary). Closing it is a no-op.
     */
    public Batch query(String sql, Object... params) {
        checkOpen();
        byte[] pcb = PowderNative.query(handle, sql, toJsonArray(params));
        return PcbReader.decode(pcb);
    }

    /**
     * Zero-copy query: the returned {@link Batch} aliases native memory via a
     * direct {@code ByteBuffer}, skipping the JNI boundary copy. The caller
     * <strong>must</strong> close it (use try-with-resources) to release the
     * allocation, and must not read columns afterwards.
     *
     * <pre>try (Batch b = db.queryDirect("SELECT * FROM t")) { ... }</pre>
     */
    public Batch queryDirect(String sql, Object... params) {
        checkOpen();
        java.nio.ByteBuffer buf = PowderNative.queryDirect(handle, sql, toJsonArray(params));
        long address = PowderNative.bufferAddress(buf);
        try {
            return PcbReader.decode(buf, address, buf.capacity());
        } catch (RuntimeException e) {
            // Decoding failed — do not leak the native allocation.
            if (address != 0) {
                PowderNative.freeBuffer(address, buf.capacity());
            }
            throw e;
        }
    }

    /** Run a built {@link Query}. */
    public Batch run(Query query) {
        return query(query.sql(), query.params());
    }

    /** A transactional body; may throw to trigger a rollback. */
    @FunctionalInterface
    public interface TxBody {
        void run(Client tx) throws Exception;
    }

    /**
     * Run {@code body} in a transaction. The outermost call issues {@code
     * BEGIN IMMEDIATE} + {@code COMMIT}/{@code ROLLBACK}; nested calls use
     * {@code SAVEPOINT}/{@code RELEASE}/{@code ROLLBACK TO}, so an inner
     * transaction that throws rolls back only its own work.
     */
    public void transaction(TxBody body) {
        int depth = txDepth;
        String savepoint = depth > 0 ? "powder_sp_" + depth : null;
        execute(savepoint != null ? "SAVEPOINT " + savepoint : "BEGIN IMMEDIATE");
        txDepth = depth + 1;
        try {
            body.run(this);
            execute(savepoint != null ? "RELEASE " + savepoint : "COMMIT");
        } catch (Exception err) {
            try {
                if (savepoint != null) {
                    execute("ROLLBACK TO " + savepoint);
                    execute("RELEASE " + savepoint);
                } else {
                    execute("ROLLBACK");
                }
            } catch (RuntimeException ignored) {
                // Surface the original failure.
            }
            throw (err instanceof RuntimeException) ? (RuntimeException) err : new RuntimeException(err);
        } finally {
            txDepth = depth;
        }
    }

    /**
     * Build the model layer from {@code powder.schema.json} text — the same
     * operation semantics as every other Powder ORM, executed by the shared
     * Rust engine.
     */
    public Orm orm(String schemaJson) {
        checkOpen();
        return new Orm(this, schemaJson);
    }

    @Override
    public void close() {
        if (handle != 0) {
            PowderNative.close(handle);
            handle = 0;
        }
    }

    /** The native handle, for same-package extensions (the ORM). */
    long handle() {
        checkOpen();
        return handle;
    }

    private void checkOpen() {
        if (handle == 0) {
            throw new IllegalStateException("client is closed");
        }
    }

    // -- parameter marshaling: Object[] -> JSON array string ---------------

    static String toJsonArray(Object[] params) {
        if (params == null || params.length == 0) {
            return "[]";
        }
        StringBuilder sb = new StringBuilder("[");
        for (int i = 0; i < params.length; i++) {
            if (i > 0) {
                sb.append(',');
            }
            appendJson(sb, params[i]);
        }
        return sb.append(']').toString();
    }

    private static void appendJson(StringBuilder sb, Object v) {
        if (v == null) {
            sb.append("null");
        } else if (v instanceof String) {
            appendJsonString(sb, (String) v);
        } else if (v instanceof Boolean) {
            sb.append(((Boolean) v) ? "true" : "false");
        } else if (v instanceof Long || v instanceof Integer || v instanceof Short || v instanceof Byte) {
            sb.append(v.toString());
        } else if (v instanceof Double || v instanceof Float) {
            double d = ((Number) v).doubleValue();
            if (Double.isNaN(d) || Double.isInfinite(d)) {
                throw new IllegalArgumentException("cannot bind non-finite number");
            }
            sb.append(v.toString());
        } else {
            throw new IllegalArgumentException("unsupported parameter type: " + v.getClass().getName());
        }
    }

    private static void appendJsonString(StringBuilder sb, String s) {
        sb.append('"');
        for (int i = 0; i < s.length(); i++) {
            char c = s.charAt(i);
            switch (c) {
                case '"':
                    sb.append("\\\"");
                    break;
                case '\\':
                    sb.append("\\\\");
                    break;
                case '\n':
                    sb.append("\\n");
                    break;
                case '\r':
                    sb.append("\\r");
                    break;
                case '\t':
                    sb.append("\\t");
                    break;
                default:
                    if (c < 0x20) {
                        sb.append(String.format("\\u%04x", (int) c));
                    } else {
                        sb.append(c);
                    }
            }
        }
        sb.append('"');
    }
}
