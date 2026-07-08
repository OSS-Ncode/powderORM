//! NCB ("Ncode Columnar Buffer") wire format — encode / decode.
//!
//! The format is a single contiguous, self-describing byte buffer with a fixed
//! header, a fixed-size column directory, a names region, and 8-byte-aligned
//! data buffers. Because every buffer's absolute offset is recorded in the
//! directory and numeric payloads are 8-byte aligned, a reader in *any*
//! language can construct typed-array views (`Float64Array`, `memoryview.cast`)
//! straight over the transferred bytes with no per-value copy — that is the
//! zero-copy property the format is built around.
//!
//! Layout (all integers little-endian):
//! ```text
//! Header (24 bytes)
//!   0  magic            [u8; 4] = b"NCB1"
//!   4  version          u16     = 1
//!   6  flags            u16
//!   8  num_columns      u32
//!   12 num_rows         u32
//!   16 directory_offset u32
//!   20 reserved         u32
//! Directory: num_columns entries, 40 bytes each
//!   0  name_off         u32   (absolute)
//!   4  name_len         u32
//!   8  dtype            u8
//!   9  flags            u8    (bit0 = has_validity)
//!   10 reserved         u16
//!   12 validity_off     u32
//!   16 validity_len     u32
//!   20 buf1_off         u32   (numeric data | bool bitmap | utf8 offsets)
//!   24 buf1_len         u32
//!   28 buf2_off         u32   (utf8 char data; 0 otherwise)
//!   32 buf2_len         u32
//!   36 reserved         u32
//! Names region: concatenated UTF-8 column names
//! Data region: per-column buffers, each padded to an 8-byte boundary
//! ```

use crate::array::{Column, ColumnData};
use crate::batch::RecordBatch;
use crate::error::{Error, Result};
use crate::schema::{DataType, Field};

const MAGIC: &[u8; 4] = b"NCB1";
const VERSION: u16 = 1;
const HEADER_LEN: usize = 24;
const COLDIR_LEN: usize = 40;

fn align8(v: &mut Vec<u8>) {
    while v.len() % 8 != 0 {
        v.push(0);
    }
}

fn bits_len(n: usize) -> usize {
    n.div_ceil(8)
}

/// Encode a [`RecordBatch`] into an NCB byte buffer.
pub fn encode(batch: &RecordBatch) -> Vec<u8> {
    let ncols = batch.columns.len();
    let nrows = batch.num_rows;

    let dir_off = HEADER_LEN;
    let names_off = dir_off + ncols * COLDIR_LEN;

    let mut out = vec![0u8; names_off];

    // Names region.
    let mut name_spans = Vec::with_capacity(ncols);
    for col in &batch.columns {
        let off = out.len() as u32;
        out.extend_from_slice(col.field.name.as_bytes());
        name_spans.push((off, col.field.name.len() as u32));
    }
    align8(&mut out);

    // Data region — one directory entry accumulated per column.
    struct Entry {
        name_off: u32,
        name_len: u32,
        dtype: u8,
        flags: u8,
        validity_off: u32,
        validity_len: u32,
        buf1_off: u32,
        buf1_len: u32,
        buf2_off: u32,
        buf2_len: u32,
    }
    let mut entries = Vec::with_capacity(ncols);

    for (i, col) in batch.columns.iter().enumerate() {
        // Validity bitmap.
        let (validity_off, validity_len, flags) = match &col.validity {
            Some(bits) => {
                align8(&mut out);
                let off = out.len() as u32;
                out.extend_from_slice(bits);
                (off, bits.len() as u32, 1u8)
            }
            None => (0, 0, 0u8),
        };

        // Payload buffer(s).
        let (buf1_off, buf1_len, buf2_off, buf2_len) = match &col.data {
            ColumnData::Int64(values) => {
                align8(&mut out);
                let off = out.len() as u32;
                for v in values {
                    out.extend_from_slice(&v.to_le_bytes());
                }
                (off, (values.len() * 8) as u32, 0, 0)
            }
            ColumnData::Float64(values) => {
                align8(&mut out);
                let off = out.len() as u32;
                for v in values {
                    out.extend_from_slice(&v.to_le_bytes());
                }
                (off, (values.len() * 8) as u32, 0, 0)
            }
            ColumnData::Bool(values) => {
                align8(&mut out);
                let off = out.len() as u32;
                let mut byte = 0u8;
                for (j, v) in values.iter().enumerate() {
                    if *v {
                        byte |= 1 << (j % 8);
                    }
                    if j % 8 == 7 {
                        out.push(byte);
                        byte = 0;
                    }
                }
                if values.len() % 8 != 0 {
                    out.push(byte);
                }
                (off, bits_len(values.len()) as u32, 0, 0)
            }
            ColumnData::Utf8 { offsets, data } => {
                align8(&mut out);
                let off1 = out.len() as u32;
                for o in offsets {
                    out.extend_from_slice(&o.to_le_bytes());
                }
                let len1 = (offsets.len() * 4) as u32;
                align8(&mut out);
                let off2 = out.len() as u32;
                out.extend_from_slice(data);
                (off1, len1, off2, data.len() as u32)
            }
        };

        let (name_off, name_len) = name_spans[i];
        entries.push(Entry {
            name_off,
            name_len,
            dtype: col.field.data_type.code(),
            flags,
            validity_off,
            validity_len,
            buf1_off,
            buf1_len,
            buf2_off,
            buf2_len,
        });
    }

    // Trailing pad so the whole buffer is a multiple of 8 bytes — keeps every
    // buffer aligned even when concatenated and simplifies host-side asserts.
    align8(&mut out);

    // Patch header.
    out[0..4].copy_from_slice(MAGIC);
    out[4..6].copy_from_slice(&VERSION.to_le_bytes());
    out[6..8].copy_from_slice(&0u16.to_le_bytes());
    out[8..12].copy_from_slice(&(ncols as u32).to_le_bytes());
    out[12..16].copy_from_slice(&(nrows as u32).to_le_bytes());
    out[16..20].copy_from_slice(&(dir_off as u32).to_le_bytes());

    // Patch directory.
    for (i, e) in entries.iter().enumerate() {
        let base = dir_off + i * COLDIR_LEN;
        out[base..base + 4].copy_from_slice(&e.name_off.to_le_bytes());
        out[base + 4..base + 8].copy_from_slice(&e.name_len.to_le_bytes());
        out[base + 8] = e.dtype;
        out[base + 9] = e.flags;
        out[base + 12..base + 16].copy_from_slice(&e.validity_off.to_le_bytes());
        out[base + 16..base + 20].copy_from_slice(&e.validity_len.to_le_bytes());
        out[base + 20..base + 24].copy_from_slice(&e.buf1_off.to_le_bytes());
        out[base + 24..base + 28].copy_from_slice(&e.buf1_len.to_le_bytes());
        out[base + 28..base + 32].copy_from_slice(&e.buf2_off.to_le_bytes());
        out[base + 32..base + 36].copy_from_slice(&e.buf2_len.to_le_bytes());
    }

    out
}

fn ru16(b: &[u8], pos: usize) -> Result<u16> {
    let s = b
        .get(pos..pos + 2)
        .ok_or_else(|| Error::Codec("truncated: u16".into()))?;
    Ok(u16::from_le_bytes(s.try_into().unwrap()))
}

fn ru32(b: &[u8], pos: usize) -> Result<u32> {
    let s = b
        .get(pos..pos + 4)
        .ok_or_else(|| Error::Codec("truncated: u32".into()))?;
    Ok(u32::from_le_bytes(s.try_into().unwrap()))
}

fn slice(b: &[u8], off: u32, len: u32) -> Result<&[u8]> {
    let (off, len) = (off as usize, len as usize);
    b.get(off..off + len)
        .ok_or_else(|| Error::Codec(format!("buffer out of range at {off}+{len}")))
}

/// Decode an NCB byte buffer into a [`RecordBatch`].
pub fn decode(bytes: &[u8]) -> Result<RecordBatch> {
    if bytes.len() < HEADER_LEN {
        return Err(Error::Codec("buffer smaller than header".into()));
    }
    if &bytes[0..4] != MAGIC {
        return Err(Error::Codec("bad magic (not an NCB buffer)".into()));
    }
    let version = ru16(bytes, 4)?;
    if version != VERSION {
        return Err(Error::Codec(format!("unsupported NCB version {version}")));
    }
    let ncols = ru32(bytes, 8)? as usize;
    let nrows = ru32(bytes, 12)? as usize;
    let dir_off = ru32(bytes, 16)? as usize;

    let mut columns = Vec::with_capacity(ncols);
    for i in 0..ncols {
        let base = dir_off + i * COLDIR_LEN;
        if base + COLDIR_LEN > bytes.len() {
            return Err(Error::Codec("truncated directory".into()));
        }
        let name_off = ru32(bytes, base)?;
        let name_len = ru32(bytes, base + 4)?;
        let dtype = DataType::from_code(bytes[base + 8])?;
        let has_validity = bytes[base + 9] & 1 != 0;
        let validity_off = ru32(bytes, base + 12)?;
        let validity_len = ru32(bytes, base + 16)?;
        let buf1_off = ru32(bytes, base + 20)?;
        let buf1_len = ru32(bytes, base + 24)?;
        let buf2_off = ru32(bytes, base + 28)?;
        let buf2_len = ru32(bytes, base + 32)?;

        let name = std::str::from_utf8(slice(bytes, name_off, name_len)?)
            .map_err(|e| Error::Codec(format!("column name not UTF-8: {e}")))?
            .to_string();

        let validity = if has_validity {
            Some(slice(bytes, validity_off, validity_len)?.to_vec())
        } else {
            None
        };

        let data = match dtype {
            DataType::Int64 => {
                let raw = slice(bytes, buf1_off, buf1_len)?;
                let mut v = Vec::with_capacity(nrows);
                for r in 0..nrows {
                    let o = r * 8;
                    v.push(i64::from_le_bytes(raw[o..o + 8].try_into().unwrap()));
                }
                ColumnData::Int64(v)
            }
            DataType::Float64 => {
                let raw = slice(bytes, buf1_off, buf1_len)?;
                let mut v = Vec::with_capacity(nrows);
                for r in 0..nrows {
                    let o = r * 8;
                    v.push(f64::from_le_bytes(raw[o..o + 8].try_into().unwrap()));
                }
                ColumnData::Float64(v)
            }
            DataType::Bool => {
                let raw = slice(bytes, buf1_off, buf1_len)?;
                let mut v = Vec::with_capacity(nrows);
                for r in 0..nrows {
                    v.push(raw[r / 8] & (1 << (r % 8)) != 0);
                }
                ColumnData::Bool(v)
            }
            DataType::Utf8 => {
                let raw_off = slice(bytes, buf1_off, buf1_len)?;
                let mut offsets = Vec::with_capacity(nrows + 1);
                for r in 0..=nrows {
                    let o = r * 4;
                    offsets.push(u32::from_le_bytes(raw_off[o..o + 4].try_into().unwrap()));
                }
                let data = slice(bytes, buf2_off, buf2_len)?.to_vec();
                ColumnData::Utf8 { offsets, data }
            }
        };

        columns.push(Column {
            field: Field::new(name, dtype, has_validity),
            len: nrows,
            validity,
            data,
        });
    }

    let schema = crate::schema::Schema::new(columns.iter().map(|c| c.field.clone()).collect());
    Ok(RecordBatch {
        schema,
        num_rows: nrows,
        columns,
    })
}
