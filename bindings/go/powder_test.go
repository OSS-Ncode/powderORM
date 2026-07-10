package powder_test

import (
	"errors"
	"os"
	"testing"

	powder "github.com/OSS-Ncode/powderORM/bindings/go"
)

// The native library path comes from POWDER_LIB so the test runs on any
// platform / target dir.
func libPath(t *testing.T) string {
	t.Helper()
	p := os.Getenv("POWDER_LIB")
	if p == "" {
		t.Skip("POWDER_LIB not set (path to powder_ffi.dll/.so/.dylib)")
	}
	return p
}

func open(t *testing.T) *powder.Client {
	t.Helper()
	if err := powder.Load(libPath(t)); err != nil {
		t.Fatalf("Load: %v", err)
	}
	db, err := powder.Connect("sqlite::memory:")
	if err != nil {
		t.Fatalf("Connect: %v", err)
	}
	t.Cleanup(func() { db.Close() })
	return db
}

func seed(t *testing.T, db *powder.Client) {
	t.Helper()
	if _, err := db.Exec("CREATE TABLE users (id INTEGER, name TEXT, score REAL, active INTEGER)"); err != nil {
		t.Fatalf("create: %v", err)
	}
	n, err := db.Exec(
		"INSERT INTO users VALUES (?,?,?,?),(?,?,?,?),(?,?,?,?)",
		1, "alice", 9.5, 1,
		2, "bob", nil, 0,
		3, "héllo 🌍", -1.25, 1,
	)
	if err != nil {
		t.Fatalf("insert: %v", err)
	}
	if n != 3 {
		t.Fatalf("insert affected %d rows, want 3", n)
	}
}

func TestQueryDecodesPCB(t *testing.T) {
	db := open(t)
	seed(t, db)

	batch, err := db.Run(powder.Table("users").Select("id", "name", "score").OrderBy("id", "ASC"))
	if err != nil {
		t.Fatalf("query: %v", err)
	}
	if batch.NumRows() != 3 {
		t.Fatalf("NumRows = %d, want 3", batch.NumRows())
	}

	id := batch.Column("id")
	if id.Type() != powder.Int64 {
		t.Errorf("id type = %v, want int64", id.Type())
	}
	if id.Int64(0) != 1 || id.Int64(2) != 3 {
		t.Errorf("ids = %d..%d, want 1..3", id.Int64(0), id.Int64(2))
	}

	name := batch.Column("name")
	if name.Type() != powder.Utf8 {
		t.Errorf("name type = %v, want utf8", name.Type())
	}
	if got := name.String(0); got != "alice" {
		t.Errorf("name[0] = %q, want alice", got)
	}
	if got := name.String(2); got != "héllo 🌍" {
		t.Errorf("name[2] = %q, multi-byte UTF-8 not preserved", got)
	}

	score := batch.Column("score")
	if score.Type() != powder.Float64 {
		t.Errorf("score type = %v, want float64", score.Type())
	}
	if score.Float64(0) != 9.5 {
		t.Errorf("score[0] = %v, want 9.5", score.Float64(0))
	}
	if score.IsValid(1) || score.Get(1) != nil {
		t.Errorf("score[1] should be NULL via the validity bitmap")
	}
	if score.Float64(2) != -1.25 {
		t.Errorf("score[2] = %v, want -1.25", score.Float64(2))
	}

	if got := batch.Rows()[0]["name"]; got != "alice" {
		t.Errorf("Rows()[0][name] = %v", got)
	}
}

func TestBoundParameters(t *testing.T) {
	db := open(t)
	seed(t, db)

	batch, err := db.Query("SELECT name FROM users WHERE id >= ? ORDER BY id", 2)
	if err != nil {
		t.Fatalf("query: %v", err)
	}
	if batch.NumRows() != 2 {
		t.Fatalf("NumRows = %d, want 2", batch.NumRows())
	}
	if got := batch.Column("name").String(0); got != "bob" {
		t.Errorf("first = %q, want bob", got)
	}

	if _, err := db.Query("SELECT 1", struct{}{}); err == nil {
		t.Error("unsupported parameter type should fail")
	}
}

func TestErrorsSurfaceNativeMessage(t *testing.T) {
	db := open(t)
	if _, err := db.Query("SELECT * FROM nope"); err == nil {
		t.Fatal("query against a missing table should fail")
	} else if got := err.Error(); got == "" {
		t.Error("error message is empty")
	}
}

func countUsers(t *testing.T, db *powder.Client) int64 {
	t.Helper()
	b, err := db.Query("SELECT COUNT(*) AS n FROM users")
	if err != nil {
		t.Fatalf("count: %v", err)
	}
	return b.Column("n").Int64(0)
}

func TestTransactionCommitAndRollback(t *testing.T) {
	db := open(t)
	seed(t, db)

	if err := db.Transaction(func(tx *powder.Client) error {
		_, err := tx.Exec("INSERT INTO users VALUES (4, 'dave', 3.0, 1)")
		return err
	}); err != nil {
		t.Fatalf("commit tx: %v", err)
	}
	if n := countUsers(t, db); n != 4 {
		t.Fatalf("after commit = %d, want 4", n)
	}

	boom := errors.New("boom")
	err := db.Transaction(func(tx *powder.Client) error {
		if _, err := tx.Exec("INSERT INTO users VALUES (5, 'erin', 1.0, 1)"); err != nil {
			return err
		}
		return boom
	})
	if !errors.Is(err, boom) {
		t.Fatalf("transaction err = %v, want boom", err)
	}
	if n := countUsers(t, db); n != 4 {
		t.Fatalf("after rollback = %d, want 4", n)
	}
}

func TestNestedTransactionUsesSavepoint(t *testing.T) {
	db := open(t)
	seed(t, db)

	// Inner rolls back via savepoint; outer still commits.
	err := db.Transaction(func(tx *powder.Client) error {
		if _, err := tx.Exec("INSERT INTO users VALUES (6, 'frank', 1.0, 1)"); err != nil {
			return err
		}
		_ = tx.Transaction(func(inner *powder.Client) error {
			if _, err := inner.Exec("INSERT INTO users VALUES (7, 'ghost', 1.0, 1)"); err != nil {
				return err
			}
			return errors.New("inner boom")
		})
		return nil
	})
	if err != nil {
		t.Fatalf("outer tx: %v", err)
	}
	if n := countUsers(t, db); n != 4 {
		t.Fatalf("savepoint should keep frank and drop ghost: got %d, want 4", n)
	}
	b, _ := db.Query("SELECT name FROM users WHERE id = 6")
	if b.NumRows() != 1 || b.Column("name").String(0) != "frank" {
		t.Error("frank should have survived the inner rollback")
	}
}

func TestClosedClientRejectsUse(t *testing.T) {
	db := open(t)
	if err := db.Close(); err != nil {
		t.Fatalf("close: %v", err)
	}
	if err := db.Close(); err != nil {
		t.Errorf("double close should be a no-op, got %v", err)
	}
	if _, err := db.Query("SELECT 1"); err == nil {
		t.Error("query on a closed client should fail")
	}
}

func TestQueryBuilderAndColumnAccessors(t *testing.T) {
	db := open(t)
	seed(t, db)

	// Fluent builder: Filter + OrderBy + Limit + Offset.
	batch, err := db.Run(powder.Table("users").
		Select("id", "name", "score", "active").
		Filter("id >= ?", 1).
		OrderBy("id", "ASC").
		Limit(2).
		Offset(1))
	if err != nil {
		t.Fatalf("run: %v", err)
	}
	if batch.NumRows() != 2 {
		t.Fatalf("limit/offset: got %d rows, want 2", batch.NumRows())
	}

	// Column accessors across all four types.
	cols := batch.Columns()
	if len(cols) != 4 {
		t.Fatalf("columns: got %d, want 4", len(cols))
	}
	idCol := batch.Column("id")
	if idCol.Name() != "id" || idCol.Len() != 2 {
		t.Errorf("id column meta: name=%q len=%d", idCol.Name(), idCol.Len())
	}
	if got := idCol.Type().String(); got != "int64" {
		t.Errorf("id type: %q", got)
	}
	// SQLite has no boolean type; the 0/1 column arrives as int64.
	// Offset 1 → rows are bob (inactive) and héllo (active).
	act := batch.Column("active")
	if act.Type() != powder.Int64 || act.Int64(0) != 0 || act.Int64(1) != 1 {
		t.Errorf("active flags: type=%v got (%d,%d), want int64 (0,1)",
			act.Type(), act.Int64(0), act.Int64(1))
	}
	if batch.Column("score").Type().String() != "float64" {
		t.Errorf("score type: %q", batch.Column("score").Type().String())
	}
	if batch.Column("name").Type().String() != "utf8" {
		t.Errorf("name type: %q", batch.Column("name").Type().String())
	}
}
