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

## Hooks

Six lifecycle decorators for test setup and teardown. Scope is
determined by position: module top-level applies to all tests in the
file; inside a `describe()` block applies to that group only.

| Decorator | Runs | Scope |
|-----------|------|-------|
| `before_each` / `after_each` | Before/after every test | Per-test |
| `before_all` / `after_all` | Once for all tests | Per-scope |
| `wrap_each` / `wrap_all` | Generator yield splits setup/teardown | Per-test / per-scope |

::: tryke.hooks.before_each

::: tryke.hooks.before_all

::: tryke.hooks.after_each

::: tryke.hooks.after_all

::: tryke.hooks.wrap_each

::: tryke.hooks.wrap_all

---

## `Depends`

::: tryke.hooks.Depends
