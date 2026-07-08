//! A small, idiomatic SQL query builder.
//!
//! The builder is intentionally driver-agnostic: it emits a `(sql, params)`
//! pair using positional `?` placeholders, which the [`crate::Client`] then
//! hands to the backend. It exists so the language bindings can offer a fluent,
//! injection-safe API instead of forcing callers to concatenate SQL strings.

/// A bound parameter value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// SQL NULL.
    Null,
    /// 64-bit integer.
    Int(i64),
    /// 64-bit float.
    Float(f64),
    /// UTF-8 text.
    Text(String),
    /// Boolean (bound as 0/1).
    Bool(bool),
}

impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Value::Int(v)
    }
}
impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::Float(v)
    }
}
impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::Text(v.to_string())
    }
}
impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::Text(v)
    }
}
impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}

/// Sort direction for `ORDER BY`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    Asc,
    Desc,
}

/// A fluent `SELECT` builder.
///
/// ```
/// use ncode_core::query::Query;
/// let (sql, params) = Query::table("users")
///     .select(["id", "name"])
///     .filter("age >= ?", [30i64])
///     .order_by("name", ncode_core::query::Order::Asc)
///     .limit(10)
///     .build();
/// assert_eq!(sql, "SELECT id, name FROM users WHERE age >= ? ORDER BY name ASC LIMIT 10");
/// assert_eq!(params.len(), 1);
/// ```
#[derive(Debug, Clone)]
pub struct Query {
    table: String,
    columns: Vec<String>,
    filters: Vec<String>,
    params: Vec<Value>,
    order: Option<(String, Order)>,
    limit: Option<u64>,
    offset: Option<u64>,
}

impl Query {
    /// Start a query against `table`.
    pub fn table(table: impl Into<String>) -> Self {
        Self {
            table: table.into(),
            columns: Vec::new(),
            filters: Vec::new(),
            params: Vec::new(),
            order: None,
            limit: None,
            offset: None,
        }
    }

    /// Select an explicit column list (defaults to `*` when never called).
    pub fn select<I, S>(mut self, columns: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.columns = columns.into_iter().map(Into::into).collect();
        self
    }

    /// Add a `WHERE` predicate with its bound parameters.
    ///
    /// The `predicate` should contain one `?` per supplied parameter. Multiple
    /// `filter` calls are combined with `AND`.
    pub fn filter<I, V>(mut self, predicate: impl Into<String>, params: I) -> Self
    where
        I: IntoIterator<Item = V>,
        V: Into<Value>,
    {
        self.filters.push(predicate.into());
        self.params.extend(params.into_iter().map(Into::into));
        self
    }

    /// Set `ORDER BY`.
    pub fn order_by(mut self, column: impl Into<String>, order: Order) -> Self {
        self.order = Some((column.into(), order));
        self
    }

    /// Set `LIMIT`.
    pub fn limit(mut self, n: u64) -> Self {
        self.limit = Some(n);
        self
    }

    /// Set `OFFSET`.
    pub fn offset(mut self, n: u64) -> Self {
        self.offset = Some(n);
        self
    }

    /// Render the `(sql, params)` pair.
    pub fn build(self) -> (String, Vec<Value>) {
        let cols = if self.columns.is_empty() {
            "*".to_string()
        } else {
            self.columns.join(", ")
        };
        let mut sql = format!("SELECT {cols} FROM {}", self.table);
        if !self.filters.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&self.filters.join(" AND "));
        }
        if let Some((col, ord)) = &self.order {
            let dir = match ord {
                Order::Asc => "ASC",
                Order::Desc => "DESC",
            };
            sql.push_str(&format!(" ORDER BY {col} {dir}"));
        }
        if let Some(n) = self.limit {
            sql.push_str(&format!(" LIMIT {n}"));
        }
        if let Some(n) = self.offset {
            sql.push_str(&format!(" OFFSET {n}"));
        }
        (sql, self.params)
    }
}
