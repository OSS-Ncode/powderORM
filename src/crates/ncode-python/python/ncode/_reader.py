"""Zero-copy reader for the NCB ("Ncode Columnar Buffer") wire format.

The native extension returns query results as NCB ``bytes``. This module parses
the fixed header + directory and exposes each column. Numeric columns are
surfaced via ``memoryview.cast`` *views* over the transferred buffer — no
per-value copy — which is possible because the encoder 8-byte-aligns every
numeric payload.
"""

from __future__ import annotations

import struct
from dataclasses import dataclass
from enum import IntEnum
from typing import Any, Optional, Sequence

_MAGIC = b"NCB1"
_HEADER_LEN = 24
_COLDIR_LEN = 40


class DataType(IntEnum):
    INT64 = 0
    FLOAT64 = 1
    BOOL = 2
    UTF8 = 3


def _valid_at(validity: Optional[memoryview], row: int) -> bool:
    if validity is None:
        return True
    return bool(validity[row >> 3] & (1 << (row & 7)))


@dataclass
class Column:
    """A single decoded column."""

    name: str
    type: DataType
    length: int
    _validity: Optional[memoryview]
    _values: Sequence[Any]  # a memoryview (numeric/bool) or a decoded helper

    def is_valid(self, row: int) -> bool:
        return _valid_at(self._validity, row)

    def get(self, row: int) -> Any:
        if row < 0 or row >= self.length or not self.is_valid(row):
            return None
        return self._values[row]

    def to_list(self) -> list[Any]:
        return [self.get(i) for i in range(self.length)]


class _BoolView:
    """Bit-unpacking view over a bool bitmap."""

    def __init__(self, bits: memoryview):
        self._bits = bits

    def __getitem__(self, row: int) -> bool:
        return bool(self._bits[row >> 3] & (1 << (row & 7)))


class _Utf8View:
    """Offset-indexed view over a UTF-8 char-data buffer."""

    def __init__(self, offsets: Sequence[int], data: memoryview):
        self._offsets = offsets
        self._data = data

    def __getitem__(self, row: int) -> str:
        start, end = self._offsets[row], self._offsets[row + 1]
        return bytes(self._data[start:end]).decode("utf-8")


class Batch:
    """A decoded columnar result set."""

    def __init__(self, num_rows: int, columns: list[Column]):
        self.num_rows = num_rows
        self.columns = columns
        self._by_name = {c.name: c for c in columns}

    def column(self, name: str) -> Optional[Column]:
        return self._by_name.get(name)

    def to_rows(self) -> list[dict[str, Any]]:
        return [
            {c.name: c.get(r) for c in self.columns} for r in range(self.num_rows)
        ]

    def __repr__(self) -> str:
        cols = ", ".join(f"{c.name}:{c.type.name.lower()}" for c in self.columns)
        return f"<ncode.Batch rows={self.num_rows} [{cols}]>"


def decode_batch(buf: bytes) -> Batch:
    """Decode an NCB buffer into a :class:`Batch`."""
    mv = memoryview(buf)
    if mv[0:4] != _MAGIC:
        raise ValueError("not an NCB buffer (bad magic)")
    version = struct.unpack_from("<H", mv, 4)[0]
    if version != 1:
        raise ValueError(f"unsupported NCB version {version}")

    num_columns, num_rows, dir_off = struct.unpack_from("<III", mv, 8)

    columns: list[Column] = []
    for c in range(num_columns):
        d = dir_off + c * _COLDIR_LEN
        name_off, name_len = struct.unpack_from("<II", mv, d)
        dtype = DataType(mv[d + 8])
        has_validity = bool(mv[d + 9] & 1)
        validity_off, validity_len = struct.unpack_from("<II", mv, d + 12)
        buf1_off, _buf1_len, buf2_off, buf2_len = struct.unpack_from("<IIII", mv, d + 20)

        name = bytes(mv[name_off : name_off + name_len]).decode("utf-8")
        validity = (
            mv[validity_off : validity_off + validity_len] if has_validity else None
        )

        values: Sequence[Any]
        if dtype is DataType.INT64:
            # Zero-copy: reinterpret the aligned little-endian region as int64.
            values = mv[buf1_off : buf1_off + num_rows * 8].cast("q")
        elif dtype is DataType.FLOAT64:
            values = mv[buf1_off : buf1_off + num_rows * 8].cast("d")
        elif dtype is DataType.BOOL:
            n_bytes = (num_rows + 7) // 8
            values = _BoolView(mv[buf1_off : buf1_off + n_bytes])
        elif dtype is DataType.UTF8:
            offsets = struct.unpack_from(f"<{num_rows + 1}I", mv, buf1_off)
            data = mv[buf2_off : buf2_off + buf2_len]
            values = _Utf8View(offsets, data)
        else:  # pragma: no cover - guarded by DataType()
            raise ValueError(f"unsupported NCB type code {dtype}")

        columns.append(Column(name, dtype, num_rows, validity, values))

    return Batch(num_rows, columns)
