package powder_test

import (
	"testing"

	powder "github.com/OSS-Ncode/powderORM/bindings/go"
)

const ormSchema = `{
  "tables": {
    "users": {
      "columns": {
        "id":     { "type": "int", "primaryKey": true },
        "name":   { "type": "text" },
        "score":  { "type": "float", "nullable": true },
        "active": { "type": "bool" }
      }
    },
    "posts": {
      "columns": {
        "id":      { "type": "int", "primaryKey": true },
        "user_id": { "type": "int", "references": { "table": "users", "column": "id" } },
        "title":   { "type": "text" }
      }
    }
  }
}`

func openOrm(t *testing.T) (*powder.Client, *powder.Orm) {
	t.Helper()
	db := open(t)
	for _, ddl := range []string{
		"CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, score REAL, active INTEGER)",
		"CREATE TABLE posts (id INTEGER PRIMARY KEY, user_id INTEGER, title TEXT)",
	} {
		if _, err := db.Exec(ddl); err != nil {
			t.Fatalf("ddl: %v", err)
		}
	}
	orm, err := db.Orm(ormSchema)
	if err != nil {
		t.Fatalf("Orm: %v", err)
	}
	t.Cleanup(orm.Close)
	return db, orm
}

func TestOrmCrud(t *testing.T) {
	_, orm := openOrm(t)
	users := orm.Table("users")

	if n, err := users.Create(powder.M{"id": 1, "name": "alice", "score": 9.5, "active": true}); err != nil || n != 1 {
		t.Fatalf("Create: n=%d err=%v", n, err)
	}
	if n, err := users.CreateMany([]powder.M{
		{"id": 2, "name": "bob", "score": 3.0, "active": false},
		{"id": 3, "name": "carol", "score": nil, "active": true},
	}); err != nil || n != 2 {
		t.Fatalf("CreateMany: n=%d err=%v", n, err)
	}

	rows, err := users.FindMany(powder.M{
		"where":   powder.M{"OR": []powder.M{{"score": powder.M{"gt": 5}}, {"score": nil}}},
		"orderBy": powder.M{"id": "asc"},
	})
	if err != nil {
		t.Fatalf("FindMany: %v", err)
	}
	if len(rows) != 2 || rows[0]["name"] != "alice" || rows[1]["name"] != "carol" {
		t.Fatalf("FindMany rows = %v", rows)
	}
	if rows[0]["active"] != true {
		t.Fatalf("bool coercion failed: %v", rows[0]["active"])
	}

	if n, err := users.Update(powder.M{"id": 2}, powder.M{"score": 10}); err != nil || n != 1 {
		t.Fatalf("Update: n=%d err=%v", n, err)
	}
	if n, err := users.Count(powder.M{"score": powder.M{"gte": 7}}); err != nil || n != 2 {
		t.Fatalf("Count: n=%d err=%v", n, err)
	}
	if ok, err := users.Exists(powder.M{"name": powder.M{"like": "%li%"}}); err != nil || !ok {
		t.Fatalf("Exists: ok=%v err=%v", ok, err)
	}
	if _, err := users.Delete(powder.M{}); err == nil {
		t.Fatal("Delete with empty where should fail")
	}
	if n, err := users.Delete(powder.M{"id": 3}); err != nil || n != 1 {
		t.Fatalf("Delete: n=%d err=%v", n, err)
	}
	if n, err := users.DeleteAll(); err != nil || n != 2 {
		t.Fatalf("DeleteAll: n=%d err=%v", n, err)
	}
}

func TestOrmRelationsAndGroupBy(t *testing.T) {
	_, orm := openOrm(t)
	users, posts := orm.Table("users"), orm.Table("posts")

	if _, err := users.CreateMany([]powder.M{
		{"id": 1, "name": "alice", "score": 10.0, "active": true},
		{"id": 2, "name": "bob", "score": 20.0, "active": true},
	}); err != nil {
		t.Fatalf("seed users: %v", err)
	}
	if _, err := posts.CreateMany([]powder.M{
		{"id": 10, "user_id": 1, "title": "hi"},
		{"id": 11, "user_id": 1, "title": "again"},
		{"id": 12, "user_id": 2, "title": "yo"},
	}); err != nil {
		t.Fatalf("seed posts: %v", err)
	}

	// include: batched relation load (belongsTo user, hasMany posts).
	rows, err := posts.FindMany(powder.M{"include": powder.M{"user": true}, "orderBy": powder.M{"id": "asc"}})
	if err != nil {
		t.Fatalf("include: %v", err)
	}
	if rows[0]["user"].(powder.M)["name"] != "alice" || rows[2]["user"].(powder.M)["name"] != "bob" {
		t.Fatalf("include user = %v", rows)
	}
	urows, err := users.FindMany(powder.M{"include": powder.M{"posts": true}, "orderBy": powder.M{"id": "asc"}})
	if err != nil {
		t.Fatalf("include hasMany: %v", err)
	}
	if len(urows[0]["posts"].([]any)) != 2 || len(urows[1]["posts"].([]any)) != 1 {
		t.Fatalf("include posts = %v", urows)
	}

	// join: single-query belongsTo hydration.
	jrows, err := posts.FindMany(powder.M{"join": powder.M{"user": true}, "where": powder.M{"id": 10}})
	if err != nil {
		t.Fatalf("join: %v", err)
	}
	if jrows[0]["user"].(powder.M)["name"] != "alice" {
		t.Fatalf("join user = %v", jrows)
	}

	// groupBy with having on an aggregate alias.
	g, err := posts.GroupBy(powder.M{
		"by": []string{"user_id"}, "count": true,
		"having":  powder.M{"_count": powder.M{"gte": 2}},
		"orderBy": powder.M{"_count": "desc"},
	})
	if err != nil {
		t.Fatalf("GroupBy: %v", err)
	}
	if len(g) != 1 || g[0]["_count"].(float64) != 2 {
		t.Fatalf("GroupBy rows = %v", g)
	}

	// aggregate.
	v, err := users.Aggregate("max", "score", nil)
	if err != nil || v == nil || *v != 20 {
		t.Fatalf("Aggregate: v=%v err=%v", v, err)
	}
	none, err := users.Aggregate("sum", "score", powder.M{"id": powder.M{"gt": 99}})
	if err != nil || none != nil {
		t.Fatalf("Aggregate empty: v=%v err=%v", none, err)
	}
}
