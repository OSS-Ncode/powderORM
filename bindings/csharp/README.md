# powder-csharp — C# / .NET 바인딩

P/Invoke로 `powder_ffi` 네이티브 라이브러리를 호출하고, PCB 컬럼 버퍼를
관리 코드에서 디코드한다. .NET 8+.

```csharp
using Powder;

using var db = Client.Connect("sqlite::memory:");
db.Execute("CREATE TABLE t (id INTEGER, name TEXT, score REAL)");
db.Execute("INSERT INTO t VALUES (?,?,?)", 1L, "alice", 9.5);

Batch batch = db.Query("SELECT * FROM t WHERE score >= ?", 5.0);
for (int r = 0; r < batch.NumRows; r++)
    Console.WriteLine($"{batch["id"].GetInt64(r)} {batch["name"].GetString(r)}");

// 트랜잭션: 반환 시 COMMIT, 예외 시 ROLLBACK. 중첩은 세이브포인트.
db.Transaction(tx => tx.Execute("INSERT INTO t VALUES (2, 'bob', 1.0)"));
```

## 네이티브 라이브러리 위치

1. 환경변수 `POWDER_LIB`에 전체 경로 지정, 또는
2. `powder_ffi.dll` / `libpowder_ffi.so`를 앱 옆이나 로더 검색 경로에 배치.

## 빌드 & 테스트

```bash
cargo build -p powder-ffi --release
cd bindings/csharp/Powder.Tests
POWDER_LIB=<target>/release/powder_ffi.dll dotnet run
# -> csharp binding OK (17 checks)
```

- 파라미터는 `long`/`double`/`bool`/`string`/`null` (내부적으로 JSON 배열로 전달).
- `Column.Get(row)`은 박싱된 값 또는 SQL NULL이면 `null`;
  `GetInt64/GetDouble/GetBoolean/GetString`은 무박싱 경로.
- 오류는 전부 `PowderException`(엔진 메시지 포함).

## ORM

`powder.schema.json` 텍스트로 만든 모델 레이어 — 다른 모든 Powder ORM과
동일한 연산·문법(공유 Rust 엔진). 옵션은 익명 객체(또는 딕셔너리),
행은 `System.Text.Json.Nodes`:

```csharp
using var orm = db.Orm(schemaJson);
var users = orm.Table("users");
users.Create(new { id = 1, name = "alice", score = 9.5, active = true });
var rows = users.FindMany(new
{
    where = new { active = true, score = new { gte = 5 } },
    orderBy = new { score = "desc" },
    limit = 10,
});
var posts = orm.Table("posts").FindMany(new { include = new { user = true } });
users.Update(new { id = 1 }, new { score = 10 });
users.GroupBy(new { by = new[] { "active" }, count = true,
                    having = new { _count = new { gte = 2 } } });
```

`where`는 `eq ne gt gte lt lte like in` + `AND/OR/NOT` 중첩,
`include`(배치 관계 로드)/`join`(belongsTo LEFT JOIN)을 지원한다.
