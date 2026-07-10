package com.powder;

/**
 * Raw JNI entry points into the Powder Rust core. Package-private: application
 * code uses {@link Client}. Load the native library once via
 * {@link Powder#loadLibrary(String)} before calling {@link Client#connect}.
 */
final class PowderNative {
    private PowderNative() {}

    static native long connect(String url);

    static native long execute(long handle, String sql, String paramsJson);

    /** PCB payload copied into a JVM {@code byte[]}. */
    static native byte[] query(long handle, String sql, String paramsJson);

    /**
     * PCB payload as a direct {@link java.nio.ByteBuffer} aliasing native
     * memory — no boundary copy. Must be released with {@link #freeBuffer}.
     */
    static native java.nio.ByteBuffer queryDirect(long handle, String sql, String paramsJson);

    /** Native address behind a direct buffer returned by {@link #queryDirect}. */
    static native long bufferAddress(java.nio.ByteBuffer buffer);

    /** Reclaim the allocation behind a {@link #queryDirect} buffer. */
    static native void freeBuffer(long address, long length);

    static native void close(long handle);

    /** Parse powder.schema.json text into a native ORM schema handle. */
    static native long ormSchemaNew(String schemaJson);

    /** Free a handle from {@link #ormSchemaNew} (at most once). */
    static native void ormSchemaFree(long schema);

    /** Run a mutation/count ORM op; returns the affected/row count. */
    static native long ormExecute(long handle, long schema, String opJson);

    /** Run a row-returning ORM op; returns its JSON result. */
    static native String ormFindJson(long handle, long schema, String opJson);
}
