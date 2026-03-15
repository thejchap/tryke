# tryke

Tryke is a fast Python test runner with zero runtime dependencies, per-assertion
diagnostics, and a clean decorator-based API.

## Highlights

- Watch mode
- Native `async` support
- Fast test discovery
- In-source testing

## Getting started

Write a test.

```python
from tryke import expect, test, describe

def add(a: int, b: int) -> int:
    return a + b

with describe("add"):
    @test("1 + 1")
    def test_basic():
        expect(1 + 1).to_equal(2)
```

Run your tests.

```bash
uvx tryke test
```

## Installation

See the [installation](guides/installation.md) documentation.
