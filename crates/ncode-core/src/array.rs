//! In-memory columnar arrays and the builders used to assemble them.
//!
//! Each [`Column`] keeps its values in a single contiguous Rust buffer plus an
//! optional validity bitmap — the same layout that is later serialized 1:1 into
//! the NCB wire format, so encoding is a set of `memcpy`s rather than a
//! per-value transform.

use crate::error::{Error, Result};
use crate::schema::{DataType, Field};

/// Column payload, one variant per [`DataType`].
#[derive(Debug, Clone, PartialEq)]
pub enum ColumnData {
    /// Packed `i64` values.
    Int64(Vec<i64>),
    /// Packed `f64` values.
    Float64(Vec<f64>),
    /// Logical booleans (bit-packed only at encode time).
    Bool(Vec<bool>),
    /// UTF-8 strings as `(offsets, data)`; `offsets.len() == len + 1`.
    Utf8 { offsets: Vec<u32>, data: Vec<u8> },
}

/// A fully built column: field metadata, length, optional validity, and data.
#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    /// Field describing this column.
    pub field: Field,
    /// Number of logical values.
    pub len: usize,
    /// Validity bitmap (LSB-first, 1 = valid). `None` means "all valid".
    pub validity: Option<Vec<u8>>,
    /// The column payload.
    pub data: ColumnData,
}

impl Column {
    /// Read an `i64` at `row`, or `None` if the slot is null / out of range.
    pub fn i64(&self, row: usize) -> Option<i64> {
        if !self.is_valid(row) {
            return None;
        }
        match &self.data {
            ColumnData::Int64(v) => v.get(row).copied(),
            _ => None,
        }
    }

    /// Read an `f64` at `row`.
    pub fn f64(&self, row: usize) -> Option<f64> {
        if !self.is_valid(row) {
            return None;
        }
        match &self.data {
            ColumnData::Float64(v) => v.get(row).copied(),
            _ => None,
        }
    }

    /// Read a `bool` at `row`.
    pub fn bool(&self, row: usize) -> Option<bool> {
        if !self.is_valid(row) {
            return None;
        }
        match &self.data {
            ColumnData::Bool(v) => v.get(row).copied(),
            _ => None,
        }
    }

    /// Read a `&str` at `row`.
    pub fn str(&self, row: usize) -> Option<&str> {
        if !self.is_valid(row) || row >= self.len {
            return None;
        }
        match &self.data {
            ColumnData::Utf8 { offsets, data } => {
                let start = *offsets.get(row)? as usize;
                let end = *offsets.get(row + 1)? as usize;
                std::str::from_utf8(&data[start..end]).ok()
            }
            _ => None,
        }
    }

    /// Whether the slot at `row` holds a value (vs. NULL).
    pub fn is_valid(&self, row: usize) -> bool {
        if row >= self.len {
            return false;
        }
        match &self.validity {
            None => true,
            Some(bits) => bits[row / 8] & (1 << (row % 8)) != 0,
        }
    }
}

/// A bit-packed validity/boolean bitmap builder (LSB-first).
#[derive(Debug, Default)]
struct BitmapBuilder {
    bits: Vec<u8>,
    len: usize,
    any_unset: bool,
}

impl BitmapBuilder {
    fn push(&mut self, value: bool) {
        if self.len % 8 == 0 {
            self.bits.push(0);
        }
        if value {
            self.bits[self.len / 8] |= 1 << (self.len % 8);
        } else {
            self.any_unset = true;
        }
        self.len += 1;
    }

    fn into_vec(self) -> Vec<u8> {
        self.bits
    }
}

/// Incrementally builds a single [`Column`], one cell at a time.
///
/// The builder tracks validity automatically: it only materializes a validity
/// bitmap if at least one NULL was pushed, so non-nullable columns pay nothing.
#[derive(Debug)]
pub enum ColumnBuilder {
    Int64 {
        values: Vec<i64>,
        validity: BitmapBuilderWrap,
    },
    Float64 {
        values: Vec<f64>,
        validity: BitmapBuilderWrap,
    },
    Bool {
        values: Vec<bool>,
        validity: BitmapBuilderWrap,
    },
    Utf8 {
        offsets: Vec<u32>,
        data: Vec<u8>,
        validity: BitmapBuilderWrap,
    },
}

/// Newtype wrapper so the private [`BitmapBuilder`] can live in public enum
/// fields without leaking its internals.
#[derive(Debug, Default)]
pub struct BitmapBuilderWrap(BitmapBuilder);

impl ColumnBuilder {
    /// Create an empty builder for the given type.
    pub fn new(data_type: DataType) -> Self {
        match data_type {
            DataType::Int64 => ColumnBuilder::Int64 {
                values: Vec::new(),
                validity: BitmapBuilderWrap::default(),
            },
            DataType::Float64 => ColumnBuilder::Float64 {
                values: Vec::new(),
                validity: BitmapBuilderWrap::default(),
            },
            DataType::Bool => ColumnBuilder::Bool {
                values: Vec::new(),
                validity: BitmapBuilderWrap::default(),
            },
            DataType::Utf8 => ColumnBuilder::Utf8 {
                offsets: vec![0],
                data: Vec::new(),
                validity: BitmapBuilderWrap::default(),
            },
        }
    }

    /// Push an integer value (coercing to float for `Float64` columns).
    pub fn push_i64(&mut self, v: i64) -> Result<()> {
        match self {
            ColumnBuilder::Int64 { values, validity } => {
                values.push(v);
                validity.0.push(true);
            }
            ColumnBuilder::Float64 { values, validity } => {
                values.push(v as f64);
                validity.0.push(true);
            }
            other => return Err(other.mismatch("i64")),
        }
        Ok(())
    }

    /// Push a float value.
    pub fn push_f64(&mut self, v: f64) -> Result<()> {
        match self {
            ColumnBuilder::Float64 { values, validity } => {
                values.push(v);
                validity.0.push(true);
            }
            other => return Err(other.mismatch("f64")),
        }
        Ok(())
    }

    /// Push a boolean value.
    pub fn push_bool(&mut self, v: bool) -> Result<()> {
        match self {
            ColumnBuilder::Bool { values, validity } => {
                values.push(v);
                validity.0.push(true);
            }
            other => return Err(other.mismatch("bool")),
        }
        Ok(())
    }

    /// Push a string value.
    pub fn push_str(&mut self, v: &str) -> Result<()> {
        match self {
            ColumnBuilder::Utf8 {
                offsets,
                data,
                validity,
            } => {
                data.extend_from_slice(v.as_bytes());
                offsets.push(data.len() as u32);
                validity.0.push(true);
            }
            other => return Err(other.mismatch("utf8")),
        }
        Ok(())
    }

    /// Push a NULL, emitting the type's zero/empty placeholder into the buffer.
    pub fn push_null(&mut self) {
        match self {
            ColumnBuilder::Int64 { values, validity } => {
                values.push(0);
                validity.0.push(false);
            }
            ColumnBuilder::Float64 { values, validity } => {
                values.push(0.0);
                validity.0.push(false);
            }
            ColumnBuilder::Bool { values, validity } => {
                values.push(false);
                validity.0.push(false);
            }
            ColumnBuilder::Utf8 {
                offsets,
                data,
                validity,
            } => {
                offsets.push(data.len() as u32);
                validity.0.push(false);
            }
        }
    }

    /// Finalize into a [`Column`] with the given name.
    pub fn finish(self, name: impl Into<String>) -> Column {
        let name = name.into();
        match self {
            ColumnBuilder::Int64 { values, validity } => Self::assemble(
                name,
                DataType::Int64,
                values.len(),
                validity,
                ColumnData::Int64(values),
            ),
            ColumnBuilder::Float64 { values, validity } => Self::assemble(
                name,
                DataType::Float64,
                values.len(),
                validity,
                ColumnData::Float64(values),
            ),
            ColumnBuilder::Bool { values, validity } => Self::assemble(
                name,
                DataType::Bool,
                values.len(),
                validity,
                ColumnData::Bool(values),
            ),
            ColumnBuilder::Utf8 {
                offsets,
                data,
                validity,
            } => {
                let len = offsets.len() - 1;
                Self::assemble(
                    name,
                    DataType::Utf8,
                    len,
                    validity,
                    ColumnData::Utf8 { offsets, data },
                )
            }
        }
    }

    fn assemble(
        name: String,
        data_type: DataType,
        len: usize,
        validity: BitmapBuilderWrap,
        data: ColumnData,
    ) -> Column {
        let nullable = validity.0.any_unset;
        let validity = if nullable {
            Some(validity.0.into_vec())
        } else {
            None
        };
        Column {
            field: Field::new(name, data_type, nullable),
            len,
            validity,
            data,
        }
    }

    fn mismatch(&self, pushed: &str) -> Error {
        let expected = match self {
            ColumnBuilder::Int64 { .. } => "int64",
            ColumnBuilder::Float64 { .. } => "float64",
            ColumnBuilder::Bool { .. } => "bool",
            ColumnBuilder::Utf8 { .. } => "utf8",
        };
        Error::Type {
            column: "<builder>".into(),
            message: format!("cannot push {pushed} into a {expected} column"),
        }
    }
}
