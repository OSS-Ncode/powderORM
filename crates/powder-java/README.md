# @powder/java — JNI 바인딩

Powder 엔진의 Java 클라이언트. 네이티브 레이어(Rust, `jni` 크레이트)가 async 연결을 소유하고 쿼리 결과를 원시 PCB 바이트 버퍼로 반환하며, 순수 Java `PcbReader`가 이를 타입 있는 컬럼으로 변환한다. Node(napi)·Python(PyO3) 바인딩과 같은 구조다.

## 빌드

```bash
# 1. 네이티브 cdylib  ->  <target>/release/powder_java.{dll,so,dylib}
cargo build -p powder-java --release

# 2. Java 클래스
cd crates/powder-java
javac -d out java/com/powder/*.java java/PowderTest.java

# 3. e2e 테스트 실행 (네이티브 라이브러리 경로를 인자로 전달)
java -cp out PowderTest <target>/release/powder_java.dll
#   -> java jni OK (17 checks)
```

Linux/macOS에서는 라이브러리가 `libpowder_java.so` / `libpowder_java.dylib`다. `java.library.path`에 올려두고 절대 경로 대신 `Powder.loadLibraryByName("powder_java")`를 쓰면 된다.

## 사용법

```java
import com.powder.*;

Powder.loadLibrary("/path/to/powder_java.dll");
try (Client db = Powder.connect("sqlite::memory:")) {
    db.execute("CREATE TABLE users (id INTEGER, name TEXT, score REAL)");
    db.execute("INSERT INTO users VALUES (?,?,?)", 1L, "alice", 9.5);

    Batch batch = db.run(Query.table("users").select("id", "name").order("id"));
    Column name = batch.column("name");
    for (int r = 0; r < batch.numRows(); r++) {
        System.out.println(name.getString(r));
    }

    // 트랜잭션 (중첩 호출은 savepoint 사용).
    db.transaction(tx -> {
        tx.execute("INSERT INTO users VALUES (2, 'bob', 7.0)");
    });
}
```

## 참고

- 바인딩 파라미터는 `Long`/`Integer`, `Double`/`Float`, `String`, `Boolean`, `null`을 받는다. JNI 경계를 JSON 배열 문자열로 넘어가므로, 객체 배열 리플렉션 없이 네이티브 표면이 좁게 유지된다.

### 복사 vs. zero-copy

| 메서드 | 저장소 | 경계 복사 | close 필요 |
|---|---|---|---|
| `query(...)` | JVM `byte[]` | 1회 복사 | 아니오 (close는 no-op) |
| `queryDirect(...)` | 네이티브 메모리 위의 direct `ByteBuffer` | **없음** | **예** |

`queryDirect`는 Rust 할당을 앨리어스하는 `DirectByteBuffer`를 JVM에 넘기므로, 큰 결과 집합도 경계 복사 비용을 내지 않는다. 그 메모리는 배치가 소유한다 — try-with-resources를 쓰고, close 이후에는 컬럼을 읽지 말 것:

```java
try (Batch b = db.queryDirect("SELECT * FROM users")) {
    // 여기서 컬럼을 읽는다
}
```

두 경로 모두 같은 `PcbReader`로 디코드하며 동일한 행을 만든다; 숫자 접근은 어느 쪽이든 리틀 엔디언 버퍼에서 바로 읽는다.

## ORM

`powder.schema.json` 텍스트로 만든 모델 레이어 — 다른 모든 Powder ORM과
동일한 연산·문법(공유 Rust 엔진). 옵션은 `Map`, 행은
`List<Map<String, Object>>`:

```java
try (Orm orm = db.orm(schemaJson)) {
    Orm.Table users = orm.table("users");
    users.create(Map.of("id", 1, "name", "alice", "score", 9.5, "active", true));
    List<Map<String, Object>> rows = users.findMany(Map.of(
        "where", Map.of("active", true, "score", Map.of("gte", 5)),
        "orderBy", Map.of("score", "desc"),
        "limit", 10));
    orm.table("posts").findMany(Map.of("include", Map.of("user", true)));
    users.update(Map.of("id", 1), Map.of("score", 10));
    users.groupBy(Map.of("by", List.of("active"), "count", true,
                         "having", Map.of("_count", Map.of("gte", 2))));
}
```

`where`는 `eq ne gt gte lt lte like in` + `AND/OR/NOT` 중첩,
`include`(배치 관계 로드)/`join`(belongsTo LEFT JOIN)을 지원한다.
다중 컬럼 `orderBy`는 `Map.of` 대신 `LinkedHashMap`으로(순서 보존).
