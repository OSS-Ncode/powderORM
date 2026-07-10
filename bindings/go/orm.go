package powder

import (
	"encoding/json"
	"errors"
	"fmt"
)

// M is shorthand for the JSON-shaped maps the ORM takes and returns.
type M = map[string]any

// Orm is the model layer over a Client: the same operation semantics as the
// TS/Python ORMs, executed by the shared Rust engine. Build one from the
// `powder.schema.json` text; then address tables by name.
//
//	orm, err := db.Orm(schemaJSON)
//	users := orm.Table("users")
//	rows, err := users.FindMany(powder.M{
//	    "where":   powder.M{"active": true, "score": powder.M{"gte": 5}},
//	    "orderBy": powder.M{"score": "desc"},
//	    "limit":   10,
//	})
type Orm struct {
	client *Client
	schema uintptr
}

// Orm parses a `powder.schema.json` document and returns the model layer.
func (c *Client) Orm(schemaJSON string) (*Orm, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.handle == 0 {
		return nil, errors.New("powder: client is closed")
	}
	h, err := nativeOrmSchemaNew(schemaJSON)
	if err != nil {
		return nil, err
	}
	return &Orm{client: c, schema: h}, nil
}

// Close frees the parsed schema. Safe to call more than once.
func (o *Orm) Close() {
	if o.schema != 0 {
		nativeOrmSchemaFree(o.schema)
		o.schema = 0
	}
}

// Table returns a handle for one table's CRUD surface.
func (o *Orm) Table(name string) *OrmTable {
	return &OrmTable{orm: o, name: name}
}

// OrmTable is the unified CRUD surface of Powder ORM for one table.
type OrmTable struct {
	orm  *Orm
	name string
}

func (t *OrmTable) op(name string, extra M) (string, error) {
	op := M{"op": name, "table": t.name}
	for k, v := range extra {
		if v != nil {
			op[k] = v
		}
	}
	b, err := json.Marshal(op)
	if err != nil {
		return "", fmt.Errorf("powder: cannot encode op: %w", err)
	}
	return string(b), nil
}

func (t *OrmTable) execute(name string, extra M) (int64, error) {
	c := t.orm.client
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.handle == 0 || t.orm.schema == 0 {
		return 0, errors.New("powder: client or orm is closed")
	}
	op, err := t.op(name, extra)
	if err != nil {
		return 0, err
	}
	return nativeOrmExecute(c.handle, t.orm.schema, op)
}

func (t *OrmTable) findJSON(name string, extra M, out any) error {
	c := t.orm.client
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.handle == 0 || t.orm.schema == 0 {
		return errors.New("powder: client or orm is closed")
	}
	op, err := t.op(name, extra)
	if err != nil {
		return err
	}
	raw, err := nativeOrmFindJSON(c.handle, t.orm.schema, op)
	if err != nil {
		return err
	}
	return json.Unmarshal([]byte(raw), out)
}

// FindMany returns rows matching opts (`where`, `orderBy`, `limit`, `offset`,
// `include`, `join` — same keys as the TS/Python ORMs). Pass nil for all rows.
func (t *OrmTable) FindMany(opts M) ([]M, error) {
	var rows []M
	if err := t.findJSON("findMany", opts, &rows); err != nil {
		return nil, err
	}
	return rows, nil
}

// FindFirst returns the first matching row, or nil.
func (t *OrmTable) FindFirst(opts M) (M, error) {
	var row M
	if err := t.findJSON("findFirst", opts, &row); err != nil {
		return nil, err
	}
	return row, nil
}

// All returns every row.
func (t *OrmTable) All() ([]M, error) { return t.FindMany(nil) }

// Create INSERTs one row; missing (nullable) columns are omitted.
func (t *OrmTable) Create(data M) (int64, error) {
	return t.execute("create", M{"data": data})
}

// CreateMany bulk-INSERTs with multi-row VALUES, chunked; every row must
// carry the same columns as the first.
func (t *OrmTable) CreateMany(rows []M) (int64, error) {
	return t.execute("createMany", M{"rows": rows})
}

// Update UPDATEs matching rows; returns the affected count.
func (t *OrmTable) Update(where M, data M) (int64, error) {
	return t.execute("update", M{"where": where, "data": data})
}

// Delete DELETEs matching rows. An empty where is rejected — use DeleteAll.
func (t *OrmTable) Delete(where M) (int64, error) {
	return t.execute("delete", M{"where": where})
}

// DeleteAll deletes every row (explicit opt-in).
func (t *OrmTable) DeleteAll() (int64, error) {
	return t.execute("deleteAll", nil)
}

// Count counts rows matching where (nil counts everything).
func (t *OrmTable) Count(where M) (int64, error) {
	return t.execute("count", M{"where": where})
}

// Exists reports whether at least one row matches.
func (t *OrmTable) Exists(where M) (bool, error) {
	row, err := t.FindFirst(M{"where": where, "limit": 1})
	if err != nil {
		return false, err
	}
	return row != nil, nil
}

// Aggregate runs SUM/AVG/MIN/MAX over one column; nil when no rows match.
func (t *OrmTable) Aggregate(fn, column string, where M) (*float64, error) {
	var v *float64
	if err := t.findJSON("aggregate", M{"fn": fn, "column": column, "where": where}, &v); err != nil {
		return nil, err
	}
	return v, nil
}

// GroupBy groups with aggregates (`by`, `count`, `sum`, `avg`, `min`, `max`,
// `having`, `orderBy`, `limit`, `offset`). Aggregates come back aliased
// `_count`, `_sum_<col>`, ....
func (t *OrmTable) GroupBy(opts M) ([]M, error) {
	var rows []M
	if err := t.findJSON("groupBy", opts, &rows); err != nil {
		return nil, err
	}
	return rows, nil
}
