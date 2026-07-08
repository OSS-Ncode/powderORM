/**
 * Zero-copy reader for the NCB ("Ncode Columnar Buffer") wire format.
 *
 * The native addon returns query results as an NCB `Buffer`. This reader
 * parses the fixed header + directory and exposes each column. Numeric columns
 * are surfaced as typed-array *views* directly over the transferred bytes
 * (`Float64Array` / `BigInt64Array`) — no per-value copy — whenever the buffer
 * lands on an 8-byte boundary, which the encoder guarantees for external
 * buffers.
 */

const MAGIC = 0x3142434e; // "NCB1" (0x4E 0x43 0x42 0x31) read as little-endian u32
const HEADER_LEN = 24;
const COLDIR_LEN = 40;

export enum DataType {
  Int64 = 0,
  Float64 = 1,
  Bool = 2,
  Utf8 = 3,
}

export type Scalar = bigint | number | boolean | string | null;

export interface NcodeColumn {
  readonly name: string;
  readonly type: DataType;
  readonly length: number;
  isValid(row: number): boolean;
  get(row: number): Scalar;
  /** Materialize the whole column to a JS array (copies). */
  toArray(): Scalar[];
}

const decoder = new TextDecoder();

/** Read a validity bit (LSB-first). `undefined` bitmap => all valid. */
function validAt(bitmap: Uint8Array | undefined, row: number): boolean {
  if (!bitmap) return true;
  return (bitmap[row >> 3] & (1 << (row & 7))) !== 0;
}

class ColumnImpl implements NcodeColumn {
  constructor(
    readonly name: string,
    readonly type: DataType,
    readonly length: number,
    private readonly validity: Uint8Array | undefined,
    private readonly readRaw: (row: number) => Scalar,
  ) {}

  isValid(row: number): boolean {
    return validAt(this.validity, row);
  }

  get(row: number): Scalar {
    if (row < 0 || row >= this.length || !this.isValid(row)) return null;
    return this.readRaw(row);
  }

  toArray(): Scalar[] {
    const out = new Array<Scalar>(this.length);
    for (let i = 0; i < this.length; i++) out[i] = this.get(i);
    return out;
  }
}

export interface NcodeBatch {
  readonly numRows: number;
  readonly columns: NcodeColumn[];
  column(name: string): NcodeColumn | undefined;
  /** Row-oriented view (copies) — convenient for small result sets. */
  toRows(): Record<string, Scalar>[];
}

/** Decode an NCB buffer into a columnar batch. */
export function decodeBatch(buf: Uint8Array): NcodeBatch {
  const ab = buf.buffer;
  const base = buf.byteOffset;
  const view = new DataView(ab, base, buf.byteLength);

  if (view.getUint32(0, true) !== MAGIC) {
    throw new Error("not an NCB buffer (bad magic)");
  }
  const version = view.getUint16(4, true);
  if (version !== 1) throw new Error(`unsupported NCB version ${version}`);

  const numColumns = view.getUint32(8, true);
  const numRows = view.getUint32(12, true);
  const dirOff = view.getUint32(16, true);

  // Can we make an aligned typed-array view at absolute offset `off`?
  const aligned = (off: number) => ((base + off) & 7) === 0;

  const columns: NcodeColumn[] = [];
  for (let c = 0; c < numColumns; c++) {
    const d = dirOff + c * COLDIR_LEN;
    const nameOff = view.getUint32(d, true);
    const nameLen = view.getUint32(d + 4, true);
    const dtype = view.getUint8(d + 8) as DataType;
    const hasValidity = (view.getUint8(d + 9) & 1) !== 0;
    const validityOff = view.getUint32(d + 12, true);
    const validityLen = view.getUint32(d + 16, true);
    const buf1Off = view.getUint32(d + 20, true);
    const buf2Off = view.getUint32(d + 28, true);
    const buf2Len = view.getUint32(d + 32, true);

    const name = decoder.decode(new Uint8Array(ab, base + nameOff, nameLen));
    const validity = hasValidity
      ? new Uint8Array(ab, base + validityOff, validityLen)
      : undefined;

    let readRaw: (row: number) => Scalar;
    switch (dtype) {
      case DataType.Int64: {
        if (aligned(buf1Off)) {
          const arr = new BigInt64Array(ab, base + buf1Off, numRows);
          readRaw = (r) => arr[r];
        } else {
          const dv = view;
          readRaw = (r) => dv.getBigInt64(buf1Off + r * 8, true);
        }
        break;
      }
      case DataType.Float64: {
        if (aligned(buf1Off)) {
          const arr = new Float64Array(ab, base + buf1Off, numRows);
          readRaw = (r) => arr[r];
        } else {
          const dv = view;
          readRaw = (r) => dv.getFloat64(buf1Off + r * 8, true);
        }
        break;
      }
      case DataType.Bool: {
        const bits = new Uint8Array(ab, base + buf1Off, Math.ceil(numRows / 8));
        readRaw = (r) => (bits[r >> 3] & (1 << (r & 7))) !== 0;
        break;
      }
      case DataType.Utf8: {
        const offsets = new Uint32Array(numRows + 1);
        for (let i = 0; i <= numRows; i++) {
          offsets[i] = view.getUint32(buf1Off + i * 4, true);
        }
        const data = new Uint8Array(ab, base + buf2Off, buf2Len);
        readRaw = (r) => decoder.decode(data.subarray(offsets[r], offsets[r + 1]));
        break;
      }
      default:
        throw new Error(`unsupported NCB type code ${dtype}`);
    }

    columns.push(new ColumnImpl(name, dtype, numRows, validity, readRaw));
  }

  return {
    numRows,
    columns,
    column(name) {
      return columns.find((col) => col.name === name);
    },
    toRows() {
      const rows: Record<string, Scalar>[] = [];
      for (let r = 0; r < numRows; r++) {
        const row: Record<string, Scalar> = {};
        for (const col of columns) row[col.name] = col.get(r);
        rows.push(row);
      }
      return rows;
    },
  };
}
