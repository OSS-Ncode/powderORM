# powder-go — Go 바인딩

Powder 엔진의 Go 클라이언트. `powder-ffi` 크레이트가 내보내는 안정 C ABI를 통해 Rust 코어를 호출하고, zero-copy PCB 컬럼 버퍼를 순수 Go로 디코드한다.

- **Windows**: C 툴체인 불필요 — 라이브러리를 `syscall`로 바인딩하므로 `CGO_ENABLED=0`으로 빌드된다.
- **Linux / macOS**: cgo를 통해 공유 라이브러리를 `dlopen`한다.

## 빌드 & 테스트

```bash
# 1. 네이티브 C-ABI 라이브러리
cargo build -p powder-ffi --release
#    -> <target>/release/powder_ffi.dll | libpowder_ffi.so | libpowder_ffi.dylib

# 2. 이를 대상으로 Go 테스트 실행
cd bindings/go
POWDER_LIB=<target>/release/powder_ffi.dll go test ./...
```

`POWDER_LIB`가 테스트에 네이티브 라이브러리 위치를 알려준다; 없으면 테스트는 스킵된다.

## 사용법

```go
import powder "github.com/OSS-Ncode/powderORM/bindings/go"

if err := powder.Load("/path/to/powder_ffi.dll"); err != nil { panic(err) }

db, err := powder.Connect("sqlite::memory:")
if err != nil { panic(err) }
defer db.Close()

db.Exec("CREATE TABLE users (id INTEGER, name TEXT, score REAL)")
db.Exec("INSERT INTO users VALUES (?,?,?)", 1, "alice", 9.5)

// 플루언트 빌더, 또는 바인딩 파라미터가 있는 raw SQL.
batch, err := db.Run(powder.Table("users").Select("id", "name").OrderBy("id", "ASC"))
name := batch.Column("name")
for r := 0; r < batch.NumRows(); r++ {
    fmt.Println(name.String(r))
}

// 트랜잭션; 중첩 호출은 savepoint 사용.
err = db.Transaction(func(tx *powder.Client) error {
    _, err := tx.Exec("INSERT INTO users VALUES (2, 'bob', 7.0)")
    return err
})
```

## 참고

- 바인딩 파라미터는 Go 정수, 부동소수점, `string`, `bool`, `nil`을 받는다. ABI를 JSON 배열 문자열로 넘어가므로 C 표면이 단순 포인터와 정수로 유지된다.
- `Column`은 리틀 엔디언 PCB 페이로드에서 값을 바로 읽는다; 요청하기 전까지 아무것도 구체화되지 않는다. `Batch.Rows()`는 편의용(복사) 뷰다.
- PCB 페이로드는 네이티브 메모리에서 Go 슬라이스로 한 번 복사되고 이후 GC가 소유한다 — Go는 외부 할당에 대한 포인터를 안전하게 들고 있을 수 없다.
- `Client`는 자신의 호출을 직렬화한다; Rust 코어가 단일 연결을 소유한다.

## ORM

`powder.schema.json` 텍스트로 스키마 인식 모델 레이어를 얻는다 — 다른 모든
Powder ORM과 동일한 연산·문법(공유 Rust 엔진):

```go
orm, err := db.Orm(schemaJSON)
defer orm.Close()
users := orm.Table("users")

users.Create(powder.M{"id": 1, "name": "alice", "score": 9.5, "active": true})
rows, err := users.FindMany(powder.M{
    "where":   powder.M{"active": true, "score": powder.M{"gte": 5}},
    "orderBy": powder.M{"score": "desc"},
    "limit":   10,
})
posts, _ := orm.Table("posts").FindMany(powder.M{"include": powder.M{"user": true}})
users.Update(powder.M{"id": 1}, powder.M{"score": 10})
users.Count(powder.M{"active": true})
users.GroupBy(powder.M{"by": []string{"active"}, "count": true,
    "having": powder.M{"_count": powder.M{"gte": 2}}})
```

`where`는 `eq ne gt gte lt lte like in` 연산자와 `AND`/`OR`/`NOT` 중첩,
`include`(배치 관계 로드)와 `join`(belongsTo LEFT JOIN)을 지원한다.
