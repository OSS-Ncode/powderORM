//! [`RecordBatch`]: a schema plus a set of equal-length columns.

use crate::array::Column;
use crate::codec;
use crate::error::{Error, Result};
use crate::schema::Schema;

/// A columnar result set: the unit of data transfer across the FFI boundary.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordBatch {
    /// Column schema.
    pub schema: Schema,
    /// Row count (shared by every column).
    pub num_rows: usize,
    /// Columns in schema order.
    pub columns: Vec<Column>,
}

impl RecordBatch {
    /// Construct a batch, validating that every column has `num_rows` rows.
    pub fn try_new(columns: Vec<Column>) -> Result<Self> {
        let num_rows = columns.first().map(|c| c.len).unwrap_or(0);
        for c in &columns {
            if c.len != num_rows {
                return Err(Error::Codec(format!(
                    "column `{}` has {} rows, expected {}",
                    c.field.name, c.len, num_rows
                )));
            }
        }
        let schema = Schema::new(columns.iter().map(|c| c.field.clone()).collect());
        Ok(Self {
            schema,
            num_rows,
            columns,
        })
    }

    /// Number of columns.
    pub fn num_columns(&self) -> usize {
        self.columns.len()
    }

    /// Serialize to the NCB zero-copy wire format.
    pub fn encode(&self) -> Vec<u8> {
        codec::encode(self)
    }

    /// Deserialize from the NCB wire format.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        codec::decode(bytes)
    }

    /// Look up a column by name.
    pub fn column(&self, name: &str) -> Option<&Column> {
        self.columns.iter().find(|c| c.field.name == name)
    }
}
