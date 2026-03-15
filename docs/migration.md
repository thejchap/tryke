# Migration from pytest

A side-by-side guide for moving from pytest to tryke.

## Cheat sheet

### Test functions

**pytest:**

```python
def test_addition():
    assert 1 + 1 == 2
```

**tryke:**

```python
from tryke import expect, test

@test
def addition():
    expect(1 + 1).to_equal(2)
```

The `@test` decorator replaces the `test_` prefix convention (though tryke also discovers `test_`-prefixed functions). Assertions use `expect()` instead of bare `assert`.

### Assertions

| pytest | tryke |
|--------|-------|
| `assert x == y` | `expect(x).to_equal(y)` |
| `assert x is y` | `expect(x).to_be(y)` |
| `assert x` | `expect(x).to_be_truthy()` |
| `assert not x` | `expect(x).to_be_falsy()` |
| `assert x is None` | `expect(x).to_be_none()` |
| `assert x > y` | `expect(x).to_be_greater_than(y)` |
| `assert x < y` | `expect(x).to_be_less_than(y)` |
| `assert x in y` | `expect(y).to_contain(x)` |
| `assert len(x) == n` | `expect(x).to_have_length(n)` |
| `assert x != y` | `expect(x).not_.to_equal(y)` |

### Exception testing

**pytest:**

```python
import pytest

def test_raises():
    with pytest.raises(ValueError, match="invalid"):
        int("abc")
```

**tryke:**

```python
from tryke import expect, test

@test
def raises():
    expect(lambda: int("abc")).to_raise(ValueError, match="invalid")
```

### Skipping tests

**pytest:**

```python
import pytest

@pytest.mark.skip(reason="not ready")
def test_skip():
    ...

@pytest.mark.skipif(sys.platform == "win32", reason="unix only")
def test_unix():
    ...
```

**tryke:**

```python
from tryke import test

@test.skip("not ready")
def skip():
    ...

@test.skip_if(sys.platform == "win32", reason="unix only")
def unix():
    ...
```

### Expected failures

**pytest:**

```python
@pytest.mark.xfail(reason="known bug")
def test_known():
    assert 1 == 2
```

**tryke:**

```python
@test.xfail("known bug")
def known():
    expect(1).to_equal(2)
```

### Async tests

**pytest** (requires `pytest-asyncio`):

```python
import pytest

@pytest.mark.asyncio
async def test_async():
    result = await some_coroutine()
    assert result == 42
```

**tryke** (built-in):

```python
@test
async def async_operation():
    result = await some_coroutine()
    expect(result).to_equal(42)
```

### Filtering

| pytest | tryke |
|--------|-------|
| `pytest -k "math"` | `tryke test -k "math"` |
| `pytest -m "slow"` | `tryke test -m "slow"` |
| `pytest test_file.py` | `tryke test test_file.py` |
| `pytest test_file.py::test_func` | `tryke test test_file.py` + `-k func` |

### Reporters

| pytest | tryke |
|--------|-------|
| default verbose | `tryke test` (text reporter) |
| `--tb=short` | `tryke test --reporter dot` |
| `--junit-xml=out.xml` | `tryke test --reporter junit` |
| `--json` (plugin) | `tryke test --reporter json` |

### Running changed tests

**pytest** (requires plugin):

```bash
pytest --lf  # last failed
```

**tryke** (built-in):

```bash
tryke test --changed  # tests affected by git changes
```

## What's different

### Soft assertions

Tryke assertions are **soft by default** — all assertions in a test run even if earlier ones fail. This gives you complete diagnostic output in a single run. Use `.fatal()` when you need to stop on failure:

```python
@test
def comprehensive_check():
    expect(response.status).to_equal(200).fatal()  # stop if wrong status
    expect(response.body).to_contain("success")    # soft — runs regardless
    expect(response.headers).to_contain("json")    # soft — runs regardless
```

### No fixtures (yet)

Tryke does not currently have a fixture system. Use regular Python setup/teardown patterns:

```python
@test
def with_setup():
    db = create_test_db()
    try:
        expect(db.query("SELECT 1")).to_equal(1)
    finally:
        db.close()
```

Fixtures and dependency injection are on the roadmap.

### No parametrize (yet)

Use a loop with named tests for now:

```python
for x, expected in [(1, 2), (2, 3), (3, 4)]:
    @test(name=f"increment {x}")
    def _(x=x, expected=expected):
        expect(x + 1).to_equal(expected)
```

Parametrize support is on the roadmap.
