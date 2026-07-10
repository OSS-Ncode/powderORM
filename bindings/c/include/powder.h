/*
 * powder.h — C API for the Powder engine (crates/powder-ffi).
 *
 * Link against the powder_ffi shared library. Query results arrive as a PCB
 * ("Powder Columnar Buffer") byte buffer — see docs/FORMAT.md for the layout;
 * the C++ wrapper (bindings/cpp/powder.hpp) ships a ready-made decoder.
 *
 * Error handling: fallible functions return NULL / -1 and store a thread-local
 * message readable via powder_last_error() (borrowed; valid until the next
 * failing call on the same thread) or powder_last_error_copy().
 */

#ifndef POWDER_H
#define POWDER_H

#include <stddef.h> /* size_t */
#include <stdint.h> /* int64_t */

#ifdef __cplusplus
extern "C" {
#endif

/* Opaque connection handle. */
typedef struct PowderClient PowderClient;

/*
 * Open a connection and return a handle, or NULL on failure.
 * URLs: "sqlite::memory:", "sqlite://path", a bare path, and (when the
 * library was built with the matching features) "postgres://..." /
 * "mysql://...".
 */
PowderClient *powder_connect(const char *url);

/*
 * Run a statement that returns no rows (INSERT/UPDATE/DDL; multi-statement
 * batches allowed). `params_json` is a JSON array string like
 * "[1, \"alice\", 9.5, true, null]" or NULL for none.
 * Returns rows affected, or -1 on failure.
 */
int64_t powder_execute(PowderClient *client, const char *sql, const char *params_json);

/*
 * Run a query. On success returns a malloc'd-by-Powder buffer holding
 * `*out_len` bytes of PCB data; release it with powder_free_buffer(ptr, len)
 * using the exact reported length. Returns NULL on failure.
 */
unsigned char *powder_query(PowderClient *client,
                            const char *sql,
                            const char *params_json,
                            size_t *out_len);

/*
 * Copy `len` bytes out of a powder_query buffer into caller-owned memory —
 * for hosts that must not dereference foreign pointers directly.
 */
void powder_copy_out(const unsigned char *src, size_t len, unsigned char *dst);

/* Borrowed message for the last failing call on this thread, or NULL. */
const char *powder_last_error(void);

/*
 * Copy the last error (no NUL terminator) into dst, returning its full
 * length. Call with cap=0 to query the required size; 0 means "no error".
 */
size_t powder_last_error_copy(unsigned char *dst, size_t cap);

/* Free a buffer returned by powder_query (exact ptr + len, once). */
void powder_free_buffer(unsigned char *ptr, size_t len);

/* Close a connection (at most once). */
void powder_close(PowderClient *client);

/*
 * ---- ORM ------------------------------------------------------------------
 *
 * The shared ORM engine: parse `powder.schema.json` once, then send each
 * operation as one JSON object — the same spec in every Powder language.
 *
 *   {"op":"findMany","table":"users",
 *    "where":{"active":true,"score":{"gte":5}},
 *    "orderBy":{"score":"desc"},"limit":10}
 *
 * Ops: findMany, findFirst, groupBy, aggregate (row-returning, use
 * powder_orm_find_json) and create, createMany, update, delete, deleteAll,
 * count (mutations/counts, use powder_orm_execute). `where` supports
 * eq/ne/gt/gte/lt/lte/like/in plus AND/OR/NOT nesting; findMany also takes
 * `include` (batched relation load) and `join` (single-query belongsTo).
 */

/* Opaque parsed-schema handle. */
typedef struct PowderOrmSchema PowderOrmSchema;

/* Parse powder.schema.json text; NULL on failure. Free with
 * powder_orm_schema_free. */
PowderOrmSchema *powder_orm_schema_new(const char *schema_json);

/* Free a schema handle (at most once). */
void powder_orm_schema_free(PowderOrmSchema *schema);

/*
 * Run a mutation (or count) op: create, createMany, update, delete,
 * deleteAll, count. Returns the affected/row count, or -1 on failure.
 */
int64_t powder_orm_execute(PowderClient *client,
                           const PowderOrmSchema *schema,
                           const char *op_json);

/*
 * Run a row-returning op: findMany, findFirst, groupBy, aggregate. On success
 * returns `*out_len` bytes of UTF-8 JSON (not NUL-terminated; findMany/groupBy
 * yield an array, findFirst an object or null, aggregate a number or null).
 * Release with powder_free_buffer(ptr, len). Returns NULL on failure.
 */
unsigned char *powder_orm_find_json(PowderClient *client,
                                    const PowderOrmSchema *schema,
                                    const char *op_json,
                                    size_t *out_len);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* POWDER_H */
