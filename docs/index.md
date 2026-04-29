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
  <li><span class="hl-icon hl-icon-reporters"></span>JSON, JUnit, Dot, and LLM reporters</li>
</ul>

## Getting started

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
            t.expect(users["alice"]).to_equal("alice@example.com")

    with t.describe("set"):

        @t.test("stores a new user")
        async def test_set(users: Annotated[dict[str, str], t.Depends(users)]):
            users["bob"] = "bob@example.com"
            t.expect(users["bob"]).to_equal("bob@example.com")

```

Run your tests — `tryke test --watch` for an always-on loop, `tryke test --changed`
for just what your working tree touched, or plain:

```bash
uvx tryke test
```

```text
tryke test v0.0.23

example.py:
  users
    get
      ✓ returns a stored user [0.00ms]
        ✓ expect(users["alice"]).to_equal("alice@example.com")
    set
      ✓ stores a new user [0.00ms]
        ✓ expect(users["bob"]).to_equal("bob@example.com")

 Test Files  1 passed (1)
      Tests  2 passed (2)
   Start at  08:58:39
   Duration  46.01ms (discover 6.08ms, tests 39.93ms)

  PASS
```

## Coming from pytest?

The [migration guide](https://tryke.dev/migration.html) has a side-by-side cheat
sheet and — faster — a
[copy-paste LLM prompt](https://tryke.dev/migration.html#migration-prompt) that
walks an AI coding assistant through a phased, gated pytest &rarr; Tryke
migration with discovery- and results-parity checks built in.

## License

This repository is licensed under the MIT License.

## Installation

See the [installation](guides/installation.md) documentation.
