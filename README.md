<p align="center">
  <a href="https://tryke.dev">
    <img height="170" alt="tryke-small" src="https://github.com/user-attachments/assets/39a2521a-fe9a-4235-8bb8-97b9e4f68aa7" />
  </a>
</p>
<h1 align="center">Tryke</h1>

<p align="center"><a href="https://github.com/astral-sh/ruff"><img src="https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/astral-sh/ruff/main/assets/badge/v2.json" alt="ruff" /></a> <a href="https://pypi.org/project/tryke/"><img src="https://img.shields.io/pypi/v/tryke" alt="PyPI" /></a> <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="license" /></a> <a href="https://python.org"><img src="https://img.shields.io/badge/python-3.12%20%7C%203.13%20%7C%203.14%20%7C%203.15-blue.svg" alt="python" /></a> <a href="https://github.com/thejchap/tryke/actions/workflows/release.yml"><img src="https://github.com/thejchap/tryke/actions/workflows/release.yml/badge.svg" alt="CI" /></a> <a href="https://tryke.dev/"><img src="https://img.shields.io/badge/docs-tryke.dev-blue" alt="docs" /></a></p>

<video src="https://github.com/user-attachments/assets/354e21a4-b49f-4e93-a052-df98c0dfc3ae" controls muted></video>

## Getting started

For more information, see the [documentation](https://tryke.dev/).

Write a test.

```python
from tryke import Depends, describe, expect, fixture, test


class Users:
    def __init__(self) -> None:
        self._rows: dict[int, str] = {}

    def create(self, name: str) -> int:
        user_id = len(self._rows) + 1
        self._rows[user_id] = name
        return user_id

    def get(self, user_id: int) -> str:
        return self._rows[user_id]


# Fixtures combine setup + teardown in one function. `yield` splits them;
# `per="scope"` caches the value for every test in the lexical scope.
@fixture(per="scope")
def users():
    db = Users()
    yield db
    db._rows.clear()  # Teardown: runs once after the whole describe block.


with describe("users"):

    # Dependencies are explicit and typed — `Depends(users)` resolves to
    # the `Users` return type above, no magic name-matching required.
    @test
    def create_and_get(db: Users = Depends(users)):
        user_id = db.create("alice")
        # Assertions are soft by default: all three run even if one fails,
        # so you get the full diagnostic in a single run.
        expect(user_id).to_equal(1)
        expect(db.get(user_id)).to_equal("alice")
        expect(lambda: db.get(999)).to_raise(KeyError)

    # Parametrize with `@test.cases` — each case is its own test ID,
    # labels are arbitrary strings, kwargs are statically type-checked.
    @test.cases(
        test.case("lowercase", name="alice"),
        test.case("with spaces", name="Alice Liddell"),
        test.case("unicode", name="Алиса"),
    )
    def round_trips_names(name: str, db: Users = Depends(users)):
        expect(db.get(db.create(name))).to_equal(name)

    # Native async — no `pytest-asyncio` plugin needed.
    @test
    async def async_create(db: Users = Depends(users)):
        expect(db.create("bob")).to_be_greater_than(0)

    # Skip / xfail / todo markers ship in the box.
    @test.xfail("reserved-name handling not implemented yet")
    def rejects_reserved_names(db: Users = Depends(users)):
        expect(lambda: db.create("admin")).to_raise(ValueError)
```

Run your tests — `tryke watch` for an always-on loop, `tryke test --changed`
for just what your working tree touched, or plain:

```bash
uvx tryke test
```

## Coming from pytest?

The [migration guide](https://tryke.dev/migration/) has a side-by-side cheat
sheet and — faster — a
[copy-paste LLM prompt](https://tryke.dev/migration/#migration-prompt) that
walks an AI coding assistant through a phased, gated pytest &rarr; Tryke
migration with discovery- and results-parity checks built in.

## License

This repository is licensed under the MIT License.
