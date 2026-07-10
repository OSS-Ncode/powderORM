/* Smoke test for the raw C API. Exits 0 on success, prints the failure and
 * exits 1 otherwise.
 *
 *   cl /W3 test_powder.c /I ../include /link powder_ffi.dll.lib
 */
#include <stdio.h>
#include <string.h>
#include "../include/powder.h"

static int checks = 0;

#define CHECK(cond, what)                                          \
    do {                                                           \
        checks++;                                                  \
        if (!(cond)) {                                             \
            const char *err = powder_last_error();                 \
            fprintf(stderr, "FAILED: %s (%s)\n", what,             \
                    err ? err : "no engine error");                \
            return 1;                                              \
        }                                                          \
    } while (0)

int main(void) {
    PowderClient *db = powder_connect("sqlite::memory:");
    CHECK(db != NULL, "connect");

    CHECK(powder_execute(db, "CREATE TABLE t (id INTEGER, name TEXT, score REAL)", NULL) == 0,
          "create table");
    CHECK(powder_execute(db, "INSERT INTO t VALUES (?, ?, ?), (?, ?, ?)",
                         "[1, \"alice\", 9.5, 2, \"bob\", null]") == 2,
          "insert 2 rows");

    size_t len = 0;
    unsigned char *buf = powder_query(db, "SELECT id, name, score FROM t ORDER BY id", NULL, &len);
    CHECK(buf != NULL && len > 24, "query returns a PCB buffer");
    CHECK(memcmp(buf, "PCB1", 4) == 0, "buffer starts with the PCB magic");

    /* copy-out path used by pointer-restricted hosts */
    unsigned char first4[4];
    powder_copy_out(buf, 4, first4);
    CHECK(memcmp(first4, "PCB1", 4) == 0, "powder_copy_out copies bytes");
    powder_free_buffer(buf, len);

    /* error paths */
    CHECK(powder_query(db, "SELECT * FROM missing", NULL, &len) == NULL, "bad SQL returns NULL");
    const char *err = powder_last_error();
    CHECK(err != NULL && strstr(err, "missing") != NULL, "error mentions the table");
    unsigned char small[8];
    size_t need = powder_last_error_copy(small, sizeof small);
    CHECK(need > sizeof small, "last_error_copy reports full length");

    /* ORM: schema handle + JSON ops through the shared engine */
    /* (>= 0: sqlite3_changes after DDL reports the previous statement's count) */
    CHECK(powder_execute(db,
                         "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, "
                         "score REAL, active INTEGER)",
                         NULL) >= 0,
          "create users table");
    PowderOrmSchema *schema = powder_orm_schema_new(
        "{\"tables\":{\"users\":{\"columns\":{"
        "\"id\":{\"type\":\"int\",\"primaryKey\":true},"
        "\"name\":{\"type\":\"text\"},"
        "\"score\":{\"type\":\"float\",\"nullable\":true},"
        "\"active\":{\"type\":\"bool\"}}}}}");
    CHECK(schema != NULL, "orm schema parses");

    CHECK(powder_orm_execute(db, schema,
                             "{\"op\":\"createMany\",\"table\":\"users\",\"rows\":["
                             "{\"id\":1,\"name\":\"alice\",\"score\":9.5,\"active\":true},"
                             "{\"id\":2,\"name\":\"bob\",\"score\":3.0,\"active\":false}]}") == 2,
          "orm createMany");
    CHECK(powder_orm_execute(db, schema,
                             "{\"op\":\"count\",\"table\":\"users\","
                             "\"where\":{\"score\":{\"gte\":5}}}") == 1,
          "orm count with where");

    size_t jlen = 0;
    unsigned char *json = powder_orm_find_json(
        db, schema,
        "{\"op\":\"findMany\",\"table\":\"users\","
        "\"where\":{\"active\":true},\"orderBy\":{\"id\":\"asc\"}}",
        &jlen);
    CHECK(json != NULL && jlen > 2, "orm findMany returns JSON");
    CHECK(memchr(json, 'a', jlen) != NULL && jlen > 10, "orm JSON has content");
    powder_free_buffer(json, jlen);

    CHECK(powder_orm_execute(db, schema,
                             "{\"op\":\"delete\",\"table\":\"users\",\"where\":{}}") == -1,
          "orm delete with empty where fails");
    err = powder_last_error();
    CHECK(err != NULL && strstr(err, "deleteAll") != NULL, "error suggests deleteAll");

    powder_orm_schema_free(schema);
    powder_close(db);
    printf("c binding OK (%d checks)\n", checks);
    return 0;
}
