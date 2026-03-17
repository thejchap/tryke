# Writing tests

## The `@test` decorator

Mark any function as a test with the `@test` decorator:

```python
from tryke import expect, test

@test
def addition():
    expect(1 + 1).to_equal(2)
```

tryke also discovers functions with the `test_` prefix, so existing pytest-style tests work without changes:

```python
from tryke import expect

def test_addition():
    expect(1 + 1).to_equal(2)
```

## Custom test names

Pass `name=` to give a test a human-readable label:

```python
@test(name="1 + 1 should equal 2")
def addition():
    expect(1 + 1).to_equal(2)
```

This is especially useful when generating tests in a loop:

```python
for x, expected in [(1, 2), (2, 3), (3, 4)]:
    @test(name=f"increment {x}")
    def _(x=x, expected=expected):
        expect(x + 1).to_equal(expected)
```

## Tags

Tag tests for [filtering](filtering.md) with `-m`:

```python
@test(tags=["slow", "network"])
def downloads_large_file():
    ...
```

```bash
tryke test -m "slow"
tryke test -m "not network"
```

## Assertions with `expect()`

Every assertion starts with `expect()` and chains a matcher:

```python
from tryke import expect, test

@test
def assertions():
    expect(1 + 1).to_equal(2)
    expect(None).to_be_none()
    expect("hello").to_be_truthy()
    expect([1, 2, 3]).to_contain(2)
    expect([1, 2, 3]).to_have_length(3)
```

Available matchers:

| Matcher | Checks |
|---------|--------|
| `to_equal(y)` | `x == y` |
| `to_be(y)` | `x is y` |
| `to_be_truthy()` | `bool(x) is True` |
| `to_be_falsy()` | `bool(x) is False` |
| `to_be_none()` | `x is None` |
| `to_be_greater_than(y)` | `x > y` |
| `to_be_less_than(y)` | `x < y` |
| `to_be_greater_than_or_equal(y)` | `x >= y` |
| `to_be_less_than_or_equal(y)` | `x <= y` |
| `to_contain(item)` | `item in x` |
| `to_have_length(n)` | `len(x) == n` |
| `to_match(pattern)` | Regex match on `str(x)` |
| `to_raise(exc, match=)` | Callable raises exception |

### Negation

Use `.not_` to negate any matcher:

```python
expect(1).not_.to_equal(2)
expect(None).not_.to_be_truthy()
```

### Exception testing

Pass a callable to `expect()` and use `to_raise()`:

```python
@test
def raises_on_invalid_input():
    expect(lambda: int("abc")).to_raise(ValueError, match="invalid")
```

`to_raise()` accepts an optional exception type and an optional `match=` regex pattern.

### Fatal assertions

Assertions are [soft by default](../concepts/soft-assertions.md) — all assertions in a test run even if earlier ones fail. Chain `.fatal()` to stop on failure:

```python
@test
def check_response():
    expect(response.status).to_equal(200).fatal()
    expect(response.body).to_contain("ok")
```

## Grouping tests with `describe()`

Use `describe()` to group related tests under a label:

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

The group name appears as a prefix in test output.

## Skipping tests

Skip a test unconditionally:

```python
@test.skip("not ready")
def pending():
    ...
```

Skip conditionally at import time:

```python
import sys

@test.skip_if(sys.platform == "win32", reason="unix only")
def unix_only():
    ...
```

## Todo tests

Mark a test as planned but not yet implemented. Todo tests are collected but never executed:

```python
@test.todo("implement caching layer")
def caching():
    ...
```

## Expected failures

Mark a test that is known to fail:

```python
@test.xfail("upstream bug #42")
def known_broken():
    expect(1).to_equal(2)
```

If the test passes unexpectedly, tryke reports it so you know the issue may be resolved.
