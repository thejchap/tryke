# tryke

**A test framework for Python that is modern, fast, and fun.**

Tryke is a Rust-powered test runner with zero runtime dependencies,
per-assertion diagnostics, and a clean decorator-based API.

## Install

```bash
uv add tryke
```

## Write a test

```python
from tryke import expect, test

@test
def addition():
    expect(1 + 1).to_equal(2)
```

## Run it

```bash
tryke test
```

## Highlights

- **Fast** — Rust-powered discovery and concurrent execution
- **Rich diagnostics** — per-assertion expected/received output
- **Zero dependencies** — no transitive deps in your project
- **Watch mode** — live reload on file changes
- **Server mode** — persistent workers for near-instant re-runs
- **Changed-files mode** — only run tests affected by git changes
- **Async native** — first-class async test support
- **Multiple reporters** — text, JSON, JUnit, dot, LLM

## Links

- [Quick Start](quickstart.md) — up and running in 2 minutes
- [API Reference](api.md) — full decorator and assertion API
- [Migration from pytest](migration.md) — side-by-side cheat sheet
- [Why tryke?](why.md) — honest comparison with pytest
- [GitHub](https://github.com/thejchap/tryke)
- [PyPI](https://pypi.org/project/tryke/)
