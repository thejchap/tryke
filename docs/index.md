<p align="center">
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
    <img src="https://img.shields.io/badge/python-3.12%20%7C%203.13%20%7C%203.14-blue.svg" alt="python" />
  </a>
  <a href="https://github.com/thejchap/tryke/actions/workflows/release.yml">
    <img src="https://github.com/thejchap/tryke/actions/workflows/release.yml/badge.svg" alt="CI" />
  </a>
  <a href="https://thejchap.github.io/tryke/">
    <img src="https://img.shields.io/badge/docs-thejchap.github.io%2Ftryke-blue" alt="docs" />
  </a>
</p>

# Tryke

Tryke is a fast Python test runner with zero runtime dependencies, per-assertion
diagnostics, and a clean decorator-based API.

## Highlights

- [Watch mode](guides/watch-mode.md)
- Native `async` support
- [Fast](concepts/discovery.md) test discovery
- [In-source testing](guides/writing-tests.md#in-source-testing)
- Support for [doctests](https://docs.python.org/3/library/doctest.html)
- [Client/server](concepts/client-server.md) mode for fast editor integrations
- Pretty, per-assertion diagnostics
- Filtering and marks
- [Changed mode](guides/changed-mode.md) (like [pytest-picked](https://github.com/anapaulagomes/pytest-picked))
- Concurrent tests
- [Soft assertions](concepts/soft-assertions.md)
- JSON, JUnit, Dot, and LLM reporters

## Getting started

Write a test.

```python
from tryke import expect, test, describe


def add(a: int, b: int) -> int:
    return a + b


with describe("add"):

    @test("1 + 1")
    def basic():
        expect(1 + 1).to_equal(2)
```

Run your tests.

```bash
uvx tryke test
```

## Installation

See the [installation](guides/installation.md) documentation.
