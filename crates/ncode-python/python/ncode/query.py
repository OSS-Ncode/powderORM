"""Fluent, injection-safe SQL builder — the Python mirror of the Rust builder."""

from __future__ import annotations

from typing import Optional, Union

Param = Union[int, float, str, bool, None]


class Query:
    """Build a ``SELECT`` statement fluently.

    >>> sql, params = (
    ...     Query.table("users").select("id", "name").filter("age >= ?", 30).limit(10).build()
    ... )
    >>> sql
    'SELECT id, name FROM users WHERE age >= ? LIMIT 10'
    """

    def __init__(self, table: str):
        self._table = table
        self._cols: list[str] = []
        self._wheres: list[str] = []
        self._params: list[Param] = []
        self._order: Optional[str] = None
        self._limit: Optional[int] = None
        self._offset: Optional[int] = None

    @classmethod
    def table(cls, name: str) -> "Query":
        return cls(name)

    def select(self, *columns: str) -> "Query":
        self._cols = list(columns)
        return self

    def filter(self, predicate: str, *params: Param) -> "Query":
        self._wheres.append(predicate)
        self._params.extend(params)
        return self

    def order(self, column: str, direction: str = "ASC") -> "Query":
        self._order = f"{column} {direction}"
        return self

    def limit(self, n: int) -> "Query":
        self._limit = n
        return self

    def offset(self, n: int) -> "Query":
        self._offset = n
        return self

    def build(self) -> tuple[str, list[Param]]:
        cols = ", ".join(self._cols) if self._cols else "*"
        sql = f"SELECT {cols} FROM {self._table}"
        if self._wheres:
            sql += " WHERE " + " AND ".join(self._wheres)
        if self._order:
            sql += f" ORDER BY {self._order}"
        if self._limit is not None:
            sql += f" LIMIT {self._limit}"
        if self._offset is not None:
            sql += f" OFFSET {self._offset}"
        return sql, list(self._params)
