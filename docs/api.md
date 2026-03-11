# API Reference

## `test` decorator

The `test` decorator marks a function as a test. Tryke discovers functions decorated with `@test` (or prefixed with `test_`) during collection.

### Basic usage

```python
from tryke import test

@test
def my_test():
    ...
```

### Named tests

```python
@test(name="descriptive test name")
def my_test():
    ...
```

### Tags

```python
@test(tags=["slow", "network"])
def integration_test():
    ...
```

Filter by tag at runtime with `tryke test -m "slow"`.

### `@test.skip`

Skip a test unconditionally.

```python
@test.skip
def not_ready():
    ...

@test.skip("waiting on upstream fix")
def with_reason():
    ...
```

### `@test.skip_if`

Skip a test conditionally, evaluated at import time.

```python
import sys

@test.skip_if(sys.platform == "win32", reason="unix only")
def unix_test():
    ...
```

### `@test.todo`

Mark a test as a placeholder — it will be collected but not executed.

```python
@test.todo
def future_feature():
    ...

@test.todo("implement caching layer")
def with_description():
    ...
```

### `@test.xfail`

Mark a test as expected to fail.

```python
@test.xfail
def known_bug():
    ...

@test.xfail("upstream issue #42")
def with_reason():
    ...
```

---

## `expect` assertions

The `expect` function creates an `Expectation` object with chainable assertion methods. Every assertion returns a `MatchResult`.

```python
from tryke import expect
```

### Soft assertions (default)

By default, assertions are **soft** — a failing assertion records the failure but does not stop the test. All assertions in a test run, and all failures are reported together with per-assertion diagnostics.

```python
@test
def multiple_checks():
    expect(1).to_equal(1)      # pass
    expect(2).to_equal(3)      # fail — recorded, test continues
    expect("a").to_equal("a")  # pass — still runs
```

### `.fatal()`

Call `.fatal()` on any assertion to stop the test immediately on failure.

```python
@test
def must_pass():
    expect(config).not_.to_be_none().fatal()  # stops here if None
    expect(config.value).to_equal(42)
```

### `.not_`

Negate any assertion.

```python
expect(1).not_.to_equal(2)
expect(None).not_.to_be_truthy()
```

### Assertion methods

#### `to_equal(other)`

Deep equality (`==`).

```python
expect(1 + 1).to_equal(2)
expect([1, 2]).to_equal([1, 2])
```

#### `to_be(other)`

Identity check (`is`).

```python
sentinel = object()
expect(sentinel).to_be(sentinel)
```

#### `to_be_truthy()`

Value is truthy (`bool(value) is True`).

```python
expect(1).to_be_truthy()
expect([1]).to_be_truthy()
```

#### `to_be_falsy()`

Value is falsy (`bool(value) is False`).

```python
expect(0).to_be_falsy()
expect("").to_be_falsy()
```

#### `to_be_none()`

Value is `None`.

```python
expect(None).to_be_none()
expect(result).not_.to_be_none()
```

#### `to_be_greater_than(n)`

```python
expect(5).to_be_greater_than(3)
```

#### `to_be_less_than(n)`

```python
expect(3).to_be_less_than(5)
```

#### `to_be_greater_than_or_equal(n)`

```python
expect(5).to_be_greater_than_or_equal(5)
```

#### `to_be_less_than_or_equal(n)`

```python
expect(4).to_be_less_than_or_equal(5)
```

#### `to_contain(item)`

Works on lists, strings, and any container supporting `in`.

```python
expect([1, 2, 3]).to_contain(2)
expect("hello world").to_contain("world")
```

#### `to_have_length(n)`

```python
expect([1, 2, 3]).to_have_length(3)
expect("hello").to_have_length(5)
```

#### `to_match(pattern)`

Regex match against the string representation of the value.

```python
expect("hello world").to_match(r"hello")
expect("foo123").to_match(r"\d+")
```

#### `to_raise(exc_type=None, *, match=None)`

Assert that a callable raises an exception. Wrap the expression in a lambda.

```python
expect(lambda: int("abc")).to_raise(ValueError)
expect(lambda: 1 / 0).to_raise(ZeroDivisionError, match="division")
expect(lambda: None).not_.to_raise()
```

---

## `describe` context manager

Group tests visually in output. The describe name is used as a prefix in test names during reporting.

```python
from tryke import describe, expect, test

with describe("math"):
    @test
    def addition():
        expect(1 + 1).to_equal(2)

    @test
    def subtraction():
        expect(3 - 1).to_equal(2)
```

---

## CLI Reference

### `tryke test`

```
tryke test [OPTIONS] [PATHS]...
```

| Option | Description |
|--------|-------------|
| `[PATHS]...` | File paths or `file:line` specs to restrict collection |
| `-e, --exclude <PATTERN>` | Exclude files/directories from discovery (overrides `pyproject.toml`) |
| `--collect-only` | List discovered tests without running them |
| `-k <FILTER>` | Filter by name expression (e.g. `"math and not slow"`) |
| `-m <MARKERS>` | Filter by tag expression (e.g. `"slow and not network"`) |
| `--reporter <NAME>` | Output format: `text`, `json`, `dot`, `junit`, `llm` |
| `--root <PATH>` | Project root directory |
| `--port [<PORT>]` | Connect to a running tryke server |
| `--changed` | Only run tests affected by git changes since HEAD |
| `-x, --fail-fast` | Stop after the first failure |
| `--maxfail <N>` | Stop after N failures |
| `-j, --workers <N>` | Number of worker processes |

#### Changed tests

Use `--changed` to run only tests affected by files changed since `HEAD`.

```bash
tryke test --changed
tryke test --changed -k "auth"
tryke test --changed -m "slow"
tryke graph --changed
```

Tryke determines the changed file set from git, including untracked files, then uses a static import graph to select affected tests at file granularity.

If git is unavailable, or if no changed files are found, tryke falls back to running the full test set.

Use `tryke graph --changed` to inspect the affected file graph for the current change set.

This is designed for fast incremental runs during development. It is less thorough than runtime dependency tracking tools such as `pytest-testmon`, but more dependency-aware than simple changed-test-file selection.

#### Discovery config

Configure default discovery excludes in `pyproject.toml`:

```toml
[tool.tryke]
exclude = ["benchmarks/suites", "generated"]
```

Use `-e/--exclude` on `test`, `watch`, `server`, or `graph` to override the config for that command.

### `tryke watch`

```
tryke watch [OPTIONS]
```

Same filtering and reporter options as `tryke test`. Watches for file changes and reruns automatically.

### `tryke server`

```
tryke server [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--port <PORT>` | Listen port (default: `2337`) |
| `--root <PATH>` | Project root directory |
| `-e, --exclude <PATTERN>` | Exclude files/directories from discovery (overrides `pyproject.toml`) |

Start a persistent server. Run tests against it with `tryke test --port 2337`.
