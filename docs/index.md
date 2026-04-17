# tryke

Tryke is a fast Python test runner with zero runtime dependencies, per-assertion
diagnostics, and a clean decorator-based API.

## Highlights

- [Watch mode](guides/watch-mode.md)
- Native `async` support
- [Fast](concepts/discovery.md) test discovery
- [In-source testing](guides/in-source-testing.md)
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
