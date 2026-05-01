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
from typing import Annotated

import tryke as t


# Fixtures with `per="scope"` are cached for their scope:
# - Module-level for globally defined fixtures
# - Per `describe` block for fixtures defined within a `describe` block
@t.fixture(per="scope")
def database():
    db = {}
    yield db
    db.clear()


with t.describe("users"):
    # By default, fixtures run per-test
    # Fixtures can be composed by requesting other fixtures
    @t.fixture
    def users(database: Annotated[dict[str, dict[str, str]], t.Depends(database)]):
        database["users"] = {}
        return database["users"]

    with t.describe("get"):
        # Define display labels for tests
        # Async tests are supported
        @t.test("returns a stored user")
        async def test_get(users: Annotated[dict[str, str], t.Depends(users)]):
            users["alice"] = "alice@example.com"
            # Pass a label as the second argument to expect() so reports
            # show "returns stored email" instead of the raw expression
            t.expect(users["alice"], "returns stored email").to_equal(
                "alice@example.com"
            )

    with t.describe("set"):

        @t.test("stores a new user")
        async def test_set(users: Annotated[dict[str, str], t.Depends(users)]):
            users["bob"] = "bob@example.com"
            t.expect(users["bob"], "stores email under user key").to_equal(
                "bob@example.com"
            )

```

Run your tests — `tryke test --watch` for an always-on loop, `tryke test --changed`
for just what your working tree touched, or plain:

```bash
uvx tryke test
```

```ansi
[1mtryke test[0m [2mv0.0.25[0m

example.py:
  users
    get
      [32m✓[39m returns a stored user [2m[0.00ms][0m
        [32m✓[39m [2mreturns stored email[0m
    set
      [32m✓[39m stores a new user [2m[0.00ms][0m
        [32m✓[39m [2mstores email under user key[0m

 [2mTest Files[0m  [1m[32m1 passed[39m[0m [2m(1)[0m
      [2mTests[0m  [1m[32m2 passed[39m[0m [2m(2)[0m
   [2mStart at[0m  08:58:39
   [2mDuration[0m  46.01ms [2m(discover 6.08ms, tests 39.93ms)[0m

 [1m[30;42m PASS [0m[0m
```

## Coming from pytest?

The [migration guide](https://tryke.dev/migration.html) has a side-by-side cheat
sheet and — faster — a [copy-paste LLM prompt](https://tryke.dev/migration.html#migration-prompt) that
walks an AI coding assistant through a phased, gated pytest &rarr; Tryke
migration with discovery- and results-parity checks built in.

## License

This repository is licensed under the MIT License.
