<p align="center">
  <a href="https://tryke.dev">
    <img height="170" alt="tryke-small" src="https://github.com/user-attachments/assets/39a2521a-fe9a-4235-8bb8-97b9e4f68aa7" />
  </a>
</p>
<h1 align="center">Tryke</h1>

<p align="center"><a href="https://github.com/astral-sh/ruff"><img src="https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/astral-sh/ruff/main/assets/badge/v2.json" alt="ruff" /></a> <a href="https://pypi.org/project/tryke/"><img src="https://img.shields.io/pypi/v/tryke" alt="PyPI" /></a> <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="license" /></a> <a href="https://python.org"><img src="https://img.shields.io/badge/python-3.12%20%7C%203.13%20%7C%203.14%20%7C%203.15-blue.svg" alt="python" /></a> <a href="https://github.com/thejchap/tryke/actions/workflows/release.yml"><img src="https://github.com/thejchap/tryke/actions/workflows/release.yml/badge.svg" alt="CI" /></a> <a href="https://tryke.dev/"><img src="https://img.shields.io/badge/docs-tryke.dev-blue" alt="docs" /></a></p>

<video src="https://github.com/user-attachments/assets/354e21a4-b49f-4e93-a052-df98c0dfc3ae" controls muted></video>

## Highlights

- [Fast](https://tryke.dev/concepts/discovery.html) Rust-powered test discovery
- Concurrent tests by default
- Pretty, per-assertion diagnostics
- [Soft assertions](https://tryke.dev/concepts/soft-assertions.html) (like [pytest-check](https://github.com/okken/pytest-check))
- Native `async` support — no plugin
- [Watch mode](https://tryke.dev/guides/watch-mode.html)
- [Changed mode](https://tryke.dev/guides/changed-mode.html) (like [pytest-picked](https://github.com/anapaulagomes/pytest-picked))
- [Client/server](https://tryke.dev/concepts/client-server.html) mode for fast editor integrations
- [Fixtures](https://tryke.dev/guides/writing-tests.html#fixtures) with setup / teardown and typed `Depends()` injection
- [Parametrized tests](https://tryke.dev/concepts/cases.html) via `@test.cases`
- [Grouping](https://tryke.dev/guides/writing-tests.html#grouping-tests-with-describe) with `describe()` blocks
- `skip`, `skip_if`, `xfail`, and `todo` markers
- [In-source testing](https://tryke.dev/guides/writing-tests.html#in-source-testing)
- Support for [doctests](https://docs.python.org/3/library/doctest.html)
- Filtering and marks
- [Reporters](https://tryke.dev/guides/reporters.html) — text, dot, json, junit, llm, [nextest](https://nexte.st)-style, and [pytest-sugar](https://github.com/Teemu/pytest-sugar)-style

## Getting started

Run tryke with [uvx](https://docs.astral.sh/uv/guides/tools/) to get started quickly:

```bash
uvx tryke test
```

Or, check out the [tryke playground](https://playground.tryke.dev) to try it out in your browser.

To learn more about using tryke, see the [documentation](https://tryke.dev/).

Write a test.

```python
from typing import Annotated

from tryke import Depends, describe, expect, fixture, test


@fixture(per="scope")
def database():
    db = {}
    yield db
    db.clear()


with describe("users"):

    @fixture
    def users(database: Annotated[dict[str, dict[str, str]], Depends(database)]):
        database["users"] = {}

        return database["users"]

    with describe("get"):

        @test("returns a stored user")
        async def test_get(users: Annotated[dict[str, str], Depends(users)]):
            users["alice"] = "alice@example.com"

            expect(users["alice"], name="returns stored email").to_equal(
                "alice@example.com"
            )

    with describe("set"):

        @test("stores a new user")
        async def test_set(users: Annotated[dict[str, str], Depends(users)]):
            users["bob"] = "bob@example.com"

            expect(users["bob"], name="stores email under user key").to_equal(
                "bob@example.com"
            )

```

Run the tests:

```bash
uvx tryke test # run once
uvx tryke # watch mode
```

## Coming from pytest?

The [migration guide](https://tryke.dev/migration.html) has a side-by-side cheat
sheet and a
[copy-paste LLM prompt](https://tryke.dev/migration.html#migration-prompt).

## License

This repository is licensed under the [MIT License](LICENSE).
