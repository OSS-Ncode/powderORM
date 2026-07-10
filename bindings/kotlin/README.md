# powder-kotlin — Kotlin 바인딩 (ORM 스타일 DSL)

Java(JNI) 바인딩 위에 관용적 Kotlin API를 얹는다: 체이너블 쿼리 빌더,
infix where 연산자, 트랜잭션 블록. 의존성은 `com.powder` 클래스와
`powder_java` 네이티브 라이브러리뿐.

```kotlin
import dev.powder.*

Database.connect("sqlite::memory:", libPath).use { db ->
    db.execute("CREATE TABLE users (id INTEGER, name TEXT, score REAL, active INTEGER)")

    // ORM 스타일 쓰기
    db.from("users").insert("id" to 1, "name" to "alice", "score" to 9.5, "active" to 1)

    // 체이너블 읽기 — 각 단계는 새 TableRef를 돌려주므로 부분 쿼리 공유가 안전
    val top = db.from("users")
        .select("id", "name", "score")
        .where { ("score" gte 5.0) and ("active" eq 1) }
        .orderBy("score", Order.DESC)
        .limit(10)
        .all()                                  // List<Map<String, Any?>>

    val bob = db.from("users").find("id" to 2)  // Map? — 없으면 null
    db.from("users").where { "id" eq 2 }.update("score" to 5.0)
    db.from("users").where { "id" eq 2 }.delete()
    db.from("users").count()

    // 트랜잭션: 반환 시 COMMIT, 예외 시 ROLLBACK. 중첩은 세이브포인트.
    db.transaction { tx ->
        tx.from("users").insert("id" to 3, "name" to "carol", "score" to 7.5, "active" to 1)
    }
}
```

## where DSL

`eq ne gt gte lt lte like inList` infix 연산자가 파라미터화된 SQL을 만든다.
값은 절대 SQL 텍스트에 들어가지 않고(`?` 바인딩), 식별자(테이블/컬럼)는
`[A-Za-z_][A-Za-z0-9_]*` 검증을 통과해야 한다 — 인젝션이 구조적으로 차단된다.

- `"score" eq null` → `score IS NULL`, `"score" ne null` → `IS NOT NULL`
- `"id" inList emptyList()` → 항상 거짓 (`1 = 0`)
- `and` / `or`는 괄호로 묶여 우선순위가 보존된다
- **안전 가드**: `where()` 없는 `update()`/`delete()`는 예외 — 전체 테이블
  변경은 `updateAll()`/`deleteAll()`로 명시해야 한다

## 빌드 & 테스트

```bash
cargo build -p powder-java --release
cd crates/powder-java && javac -d out java/com/powder/*.java   # Java 클래스
cd ../../bindings/kotlin
kotlinc -cp ../../crates/powder-java/out src/dev/powder/Powder.kt test/KotlinTest.kt -d out
java -cp "out;../../crates/powder-java/out;<kotlin-stdlib.jar>" KotlinTestKt <powder_java.dll>
# -> kotlin binding OK (22 checks)
```

## 스키마 인식 ORM (공유 Rust 엔진)

`from(...)` DSL과 별개로, `powder.schema.json` 텍스트로 만든 모델 레이어가
다른 모든 Powder ORM과 동일한 연산·문법을 제공한다:

```kotlin
db.orm(schemaJson).use { orm ->
    val users = orm.table("users")
    users.create(mapOf("id" to 1, "name" to "alice", "score" to 9.5, "active" to true))
    val top = users.findMany(
        where = mapOf("active" to true, "score" to mapOf("gte" to 5)),
        orderBy = mapOf("score" to "desc"),
        limit = 10,
    )
    orm.table("posts").findMany(include = mapOf("user" to true))
    users.update(mapOf("id" to 1), mapOf("score" to 10))
    users.groupBy(by = listOf("active"), count = true,
                  having = mapOf("_count" to mapOf("gte" to 2)))
}
```

`where`는 `eq ne gt gte lt lte like in` + `AND/OR/NOT` 중첩,
`include`(배치 관계 로드)/`join`(belongsTo LEFT JOIN)을 지원한다.
