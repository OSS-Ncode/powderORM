//! Schema description: the logical types carried by a [`crate::RecordBatch`].

use crate::error::{Error, Result};

/// The physical/logical types supported by the NCB columnar format.
///
/// The set is deliberately small — the four types cover the overwhelming
/// majority of relational payloads while keeping the wire format and the
/// three language readers trivial to keep in sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DataType {
    /// 64-bit signed integer, stored as a packed little-endian `i64` buffer.
    Int64 = 0,
    /// 64-bit IEEE-754 float, stored as a packed little-endian `f64` buffer.
    Float64 = 1,
    /// Boolean, stored as a bit-packed buffer (1 bit per value).
    Bool = 2,
    /// UTF-8 string, stored as a `u32` offsets buffer plus a char-data buffer.
    Utf8 = 3,
}

impl DataType {
    /// Stable wire code written into the NCB directory.
    pub fn code(self) -> u8 {
        self as u8
    }

    /// Reconstruct a `DataType` from its wire code.
    pub fn from_code(code: u8) -> Result<Self> {
        Ok(match code {
            0 => DataType::Int64,
            1 => DataType::Float64,
            2 => DataType::Bool,
            3 => DataType::Utf8,
            other => return Err(Error::Unsupported(format!("type code {other}"))),
        })
    }

    /// Human-readable name, also used by the language bindings.
    pub fn name(self) -> &'static str {
        match self {
            DataType::Int64 => "int64",
            DataType::Float64 => "float64",
            DataType::Bool => "bool",
            DataType::Utf8 => "utf8",
        }
    }
}

/// A single named column definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    /// Column name.
    pub name: String,
    /// Column type.
    pub data_type: DataType,
    /// Whether the column may contain NULLs (drives validity-bitmap emission).
    pub nullable: bool,
}

impl Field {
    /// Construct a field.
    pub fn new(name: impl Into<String>, data_type: DataType, nullable: bool) -> Self {
        Self {
            name: name.into(),
            data_type,
            nullable,
        }
    }
}

/// An ordered collection of [`Field`]s describing a record batch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Schema {
    /// Fields in column order.
    pub fields: Vec<Field>,
}

impl Schema {
    /// Build a schema from a list of fields.
    pub fn new(fields: Vec<Field>) -> Self {
        Self { fields }
    }

    /// Number of columns.
    pub fn len(&self) -> usize {
        self.fields.len()
    }

    /// Whether the schema has no columns.
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }
}
