"""End-to-end test of the Python binding: async engine + zero-copy reader."""

import asyncio

import ncode


async def _run():
    db = await ncode.connect("sqlite::memory:")
    await db.execute("CREATE TABLE users (id INTEGER, name TEXT, score REAL)")
    n = await db.execute(
        "INSERT INTO users VALUES (?, ?, ?), (?, ?, ?), (?, ?, ?)",
        [1, "alice", 9.5, 2, "bob", None, 3, "héllo 🌍", -1.25],
    )
    assert n == 3, n

    batch = await db.run(
        ncode.Query.table("users").select("id", "name", "score").order("id", "ASC")
    )

    assert batch.num_rows == 3, batch.num_rows
    assert [c.name for c in batch.columns] == ["id", "name", "score"]

    ids = batch.column("id")
    assert ids.type is ncode.DataType.INT64
    assert ids.to_list() == [1, 2, 3]

    names = batch.column("name")
    assert names.type is ncode.DataType.UTF8
    assert names.to_list() == ["alice", "bob", "héllo 🌍"]

    scores = batch.column("score")
    assert scores.type is ncode.DataType.FLOAT64
    assert scores.get(0) == 9.5
    assert scores.get(1) is None  # NULL preserved via validity bitmap
    assert scores.get(2) == -1.25

    # Prove the numeric reader is a genuine zero-copy memoryview view.
    assert scores._values.format == "d"
    assert isinstance(scores._values, memoryview)

    assert batch.to_rows()[2] == {"id": 3, "name": "héllo 🌍", "score": -1.25}
    print("python e2e OK:", batch)


def test_e2e():
    asyncio.run(_run())


if __name__ == "__main__":
    test_e2e()
