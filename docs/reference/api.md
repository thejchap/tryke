# API Reference

## `test` decorator

::: tryke.expect._TestBuilder
    options:
      show_root_heading: false
      members:
        - __call__
        - skip_if

::: tryke.expect._SkipMarker
    options:
      show_root_heading: false

::: tryke.expect._TodoMarker
    options:
      show_root_heading: false

::: tryke.expect._XfailMarker
    options:
      show_root_heading: false

---

## `expect` assertions

::: tryke.expect.expect
    options:
      show_root_heading: false

::: tryke.expect.Expectation
    options:
      show_root_heading: false

::: tryke.expect.MatchResult
    options:
      show_root_heading: false

---

## `describe` context manager

::: tryke.describe
    options:
      show_root_heading: false

---

## Fixtures

A single `@fixture` decorator handles setup and teardown, with two
granularities selected via `per=`. Scope is determined by position:
module top-level applies to all tests in the file; inside a
`describe()` block applies to that group only.

| Form | Runs | Scope |
|------|------|-------|
| `@fixture` (default: `per="test"`) | Before/after every test in scope | Per-test |
| `@fixture(per="scope")` | Once for all tests in scope | Per-(lexical-)scope |

Use `yield` to split setup and teardown in the same function. See the
[Writing tests guide](../guides/writing-tests.md#fixtures) for worked
examples.

::: tryke.hooks.fixture

---

## `Depends`

::: tryke.hooks.Depends
