# NCB — Ncode Columnar Buffer (v1)

NCB is the on-the-wire, zero-copy columnar format Ncode uses to move query
results out of the Rust core and into a host language. It is intentionally
Arrow-shaped (validity bitmaps + type-specific data buffers) but far smaller in
scope, so a reader can be written in ~120 lines in any language.

A single query result is **one contiguous byte buffer**. All integers are
**little-endian**. Every data buffer starts on an **8-byte boundary**, which is
what lets a reader alias numeric buffers as `Float64Array` / `BigInt64Array`
(Node) or `memoryview.cast('d'/'q')` (Python) with **no per-value copy**.

## Overall layout

```
┌────────────┬──────────────────┬───────────────┬──────────────────────────┐
│  Header    │    Directory     │   Names       │   Data buffers           │
│  24 bytes  │  40 × ncols      │   (utf-8)     │   (8-byte aligned each)   │
└────────────┴──────────────────┴───────────────┴──────────────────────────┘
```

## Header (24 bytes)

| Offset | Size | Field              | Notes                     |
| ------ | ---- | ------------------ | ------------------------- |
| 0      | 4    | `magic`            | ASCII `NCB1`              |
| 4      | 2    | `version`          | `1`                       |
| 6      | 2    | `flags`            | reserved, `0`             |
| 8      | 4    | `num_columns`      |                           |
| 12     | 4    | `num_rows`         |                           |
| 16     | 4    | `directory_offset` | absolute; currently `24`  |
| 20     | 4    | `reserved`         | `0`                       |

## Directory entry (40 bytes each, `num_columns` of them)

| Offset | Size | Field          | Notes                                        |
| ------ | ---- | -------------- | -------------------------------------------- |
| 0      | 4    | `name_off`     | absolute offset of the column name           |
| 4      | 4    | `name_len`     | length in bytes of the UTF-8 name            |
| 8      | 1    | `dtype`        | `0`=Int64 `1`=Float64 `2`=Bool `3`=Utf8      |
| 9      | 1    | `flags`        | bit0 = has validity bitmap                   |
| 10     | 2    | `reserved`     |                                              |
| 12     | 4    | `validity_off` | absolute; valid only if bit0 set             |
| 16     | 4    | `validity_len` | bytes = `ceil(num_rows / 8)`                 |
| 20     | 4    | `buf1_off`     | primary data buffer (see below)              |
| 24     | 4    | `buf1_len`     |                                              |
| 28     | 4    | `buf2_off`     | Utf8 char data; `0` for other types          |
| 32     | 4    | `buf2_len`     |                                              |
| 36     | 4    | `reserved`     |                                              |

## Per-type buffers

| Type    | `buf1`                                   | `buf2`            |
| ------- | ---------------------------------------- | ----------------- |
| Int64   | `num_rows × i64` (LE)                     | —                 |
| Float64 | `num_rows × f64` (LE)                     | —                 |
| Bool    | bit-packed, LSB-first, `ceil(n/8)` bytes  | —                 |
| Utf8    | `(num_rows + 1) × u32` offsets (LE)       | UTF-8 char data   |

## Validity bitmap

LSB-first: value at row `r` is valid iff `bitmap[r >> 3] & (1 << (r & 7))`. When
the `has validity` flag is unset, every value is valid (the common non-null
case pays zero bytes).

## Reading zero-copy

Because `buf1_off` is 8-byte aligned and offsets are absolute, a reader creates
a view directly over the transferred bytes:

- **Node** — `new Float64Array(buf.buffer, buf.byteOffset + buf1_off, num_rows)`
- **Python** — `memoryview(buf)[buf1_off : buf1_off + num_rows*8].cast('d')`

Both alias the engine's output; no element-wise copy occurs. String and
null-checked access still go through the offsets/validity buffers, which are
themselves slices of the same buffer.
