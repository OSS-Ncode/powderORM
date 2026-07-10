# powder-c — C API

`crates/powder-ffi`가 내보내는 안정 C ABI의 헤더와 스모크 테스트.

```c
#include "powder.h"

PowderClient *db = powder_connect("sqlite::memory:");
powder_execute(db, "CREATE TABLE t (id INTEGER, name TEXT)", NULL);
powder_execute(db, "INSERT INTO t VALUES (?, ?)", "[1, \"alice\"]");

size_t len;
unsigned char *pcb = powder_query(db, "SELECT * FROM t", NULL, &len);
/* pcb는 PCB 컬럼 버퍼 (docs/FORMAT.md) — 디코더는 C++ 래퍼 참고 */
powder_free_buffer(pcb, len);
powder_close(db);
```

## 빌드 & 테스트 (Windows / MSVC)

```bat
cargo build -p powder-ffi --release
cl /W3 /utf-8 test\test_powder.c /I include /link <target>\release\powder_ffi.dll.lib
```

Unix: `cc test/test_powder.c -Iinclude -lpowder_ffi -L<target>/release`.

- 파라미터는 JSON 배열 문자열로 전달 (`"[1, \"a\", true, null]"`).
- 오류는 NULL/-1 + `powder_last_error()`(스레드 로컬, borrowed) 또는
  `powder_last_error_copy()`.
- `powder_query` 버퍼는 반환된 길이 그대로 `powder_free_buffer`로 해제.

## ORM

스키마 핸들 하나 + JSON op 하나 — 모든 언어가 공유하는 스펙 그대로:

```c
PowderOrmSchema *schema = powder_orm_schema_new(schema_json);

powder_orm_execute(db, schema,
    "{\"op\":\"create\",\"table\":\"users\","
    "\"data\":{\"id\":1,\"name\":\"alice\",\"score\":9.5,\"active\":true}}");

size_t len = 0;
unsigned char *rows = powder_orm_find_json(db, schema,
    "{\"op\":\"findMany\",\"table\":\"users\","
    "\"where\":{\"active\":true,\"score\":{\"gte\":5}},"
    "\"orderBy\":{\"score\":\"desc\"},\"limit\":10}", &len);
/* rows = UTF-8 JSON 배열 (len 바이트, NUL 없음) */
powder_free_buffer(rows, len);
powder_orm_schema_free(schema);
```

row-returning op(`findMany`/`findFirst`/`groupBy`/`aggregate`)는
`powder_orm_find_json`, 변경/카운트(`create`/`createMany`/`update`/`delete`/
`deleteAll`/`count`)는 `powder_orm_execute`(영향 행 수 반환, 실패 -1).
