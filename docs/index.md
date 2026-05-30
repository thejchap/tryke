# Tryke

A Rust-based Python test runner with a Jest-style API.

<p class="md-badges">
  <a href="https://github.com/astral-sh/ruff">
    <img src="https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/astral-sh/ruff/main/assets/badge/v2.json" alt="ruff" />
  </a>
  <a href="https://pypi.org/project/tryke/">
    <img src="https://img.shields.io/pypi/v/tryke" alt="PyPI" />
  </a>
  <a href="https://github.com/thejchap/tryke/blob/main/LICENSE">
    <img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="license" />
  </a>
  <a href="https://python.org">
    <img src="https://img.shields.io/badge/python-3.12%20%7C%203.13%20%7C%203.14%20%7C%203.15-blue.svg" alt="python" />
  </a>
  <a href="https://github.com/thejchap/tryke/actions/workflows/release.yml">
    <img src="https://github.com/thejchap/tryke/actions/workflows/release.yml/badge.svg" alt="CI" />
  </a>
  <a href="https://tryke.dev/">
    <img src="https://img.shields.io/badge/docs-tryke.dev-blue" alt="docs" />
  </a>
</p>

<video src="https://github.com/user-attachments/assets/354e21a4-b49f-4e93-a052-df98c0dfc3ae" controls muted style="max-width: 100%; max-height: 640px;"></video>

## Highlights

<ul class="md-highlights">
  <li><span class="hl-icon hl-icon-fast"></span><a href="concepts/discovery.html">Fast</a> Rust-powered test discovery</li>
  <li><span class="hl-icon hl-icon-concurrent"></span>Concurrent tests by default</li>
  <li><span class="hl-icon hl-icon-pretty"></span>Pretty, per-assertion diagnostics</li>
  <li><span class="hl-icon hl-icon-soft"></span><a href="concepts/soft-assertions.html">Soft assertions</a> (like <a href="https://github.com/okken/pytest-check">pytest-check</a>)</li>
  <li><span class="hl-icon hl-icon-async"></span>Native <code>async</code> support — no plugin</li>
  <li><span class="hl-icon hl-icon-watch"></span><a href="guides/watch-mode.html">Watch mode</a></li>
  <li><span class="hl-icon hl-icon-changed"></span><a href="guides/changed-mode.html">Changed mode</a> (like <a href="https://github.com/anapaulagomes/pytest-picked">pytest-picked</a>)</li>
  <li><span class="hl-icon hl-icon-clientsrv"></span><a href="concepts/client-server.html">Client/server</a> mode for fast editor integrations</li>
  <li><span class="hl-icon hl-icon-fixtures"></span><a href="guides/writing-tests.html#fixtures">Fixtures</a> with setup / teardown and typed <code>Depends()</code> injection</li>
  <li><span class="hl-icon hl-icon-cases"></span><a href="concepts/cases.html">Parametrized tests</a> via <code>@test.cases</code></li>
  <li><span class="hl-icon hl-icon-describe"></span><a href="guides/writing-tests.html#grouping-tests-with-describe">Grouping</a> with <code>describe()</code> blocks</li>
  <li><span class="hl-icon hl-icon-marks"></span><code>skip</code>, <code>skip_if</code>, <code>xfail</code>, and <code>todo</code> markers</li>
  <li><span class="hl-icon hl-icon-insource"></span><a href="guides/writing-tests.html#in-source-testing">In-source testing</a></li>
  <li><span class="hl-icon hl-icon-doctests"></span>Support for <a href="https://docs.python.org/3/library/doctest.html">doctests</a></li>
  <li><span class="hl-icon hl-icon-filter"></span>Filtering and marks</li>
  <li><span class="hl-icon hl-icon-reporters"></span><a href="guides/reporters.html">Reporters</a> — text, dot, json, junit, llm, <a href="https://nexte.st">nextest</a>-style, and <a href="https://github.com/Teemu/pytest-sugar">pytest-sugar</a>-style</li>
</ul>

## Getting started

Run tryke with [uvx](https://docs.astral.sh/uv/guides/tools/) to get started quickly:

```bash
uvx tryke test
```

Or, check out the [tryke playground](https://playground.tryke.dev) to try it out in your browser.

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

This repository is licensed under the [MIT License](https://github.com/thejchap/tryke/blob/main/LICENSE).

## Installation

See the [installation](./guides/installation.md) documentation.
