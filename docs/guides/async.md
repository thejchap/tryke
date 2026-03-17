# Testing async code

Async test support is built in. No plugins or extra configuration required.

## Writing async tests

Use `@test` with `async def`:

```python
from tryke import expect, test

@test
async def fetches_data():
    result = await some_coroutine()
    expect(result).to_equal(42)
```

That's it. tryke detects async test functions and runs them on an event loop automatically.

## Comparison with pytest

pytest requires `pytest-asyncio` and a marker:

```python
import pytest

@pytest.mark.asyncio
async def test_fetches_data():
    result = await some_coroutine()
    assert result == 42
```

tryke needs neither — `@test` + `async def` is all you need.

## All decorators work with async

`skip`, `todo`, `xfail`, and `skip_if` all work the same on async tests:

```python
@test.skip("service unavailable in CI")
async def calls_external_api():
    ...

@test.xfail("race condition under investigation")
async def concurrent_writes():
    ...
```
