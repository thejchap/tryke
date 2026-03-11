# Quick Start

Get from zero to running tests in under 2 minutes.

## 1. Install

```bash
# add to your project
uv add tryke

# or install globally
uv tool install tryke@latest

# or run without installing
uvx tryke test
```

## 2. Write a test

Create a file called `test_math.py`:

```python
from tryke import expect, test

@test
def addition():
    expect(1 + 1).to_equal(2)

@test
def string_contains():
    expect("hello world").to_contain("world")
```

## 3. Run

```bash
tryke test
```

You'll see per-assertion diagnostic output showing expected vs received values for any failures.

## 4. Watch mode

Rerun tests automatically when files change:

```bash
tryke watch
```

## 5. Server mode

For near-instant re-runs, start a persistent server:

```bash
# terminal 1: start the server
tryke server

# terminal 2: run tests against it
tryke test --port 2337
```

The server keeps Python workers alive and caches test discovery, so subsequent runs skip startup overhead.

## 6. Filter tests

```bash
# by name
tryke test -k "addition"

# by marker/tag
tryke test -m "slow"

# only tests affected by git changes
tryke test --changed
```

## 7. Async tests

Async tests work out of the box:

```python
import asyncio
from tryke import expect, test

@test
async def async_operation():
    await asyncio.sleep(0.01)
    expect(True).to_be_truthy()
```

## 8. Named tests and markers

```python
from tryke import expect, test

@test(name="validates email format")
def email_validation():
    expect("user@example.com").to_match(r".+@.+\..+")

@test.skip("waiting on upstream fix")
def pending_feature():
    pass

@test.todo("implement caching")
def caching():
    pass

@test.xfail("known bug #42")
def known_failure():
    expect(1).to_equal(2)
```

## Next steps

- [API Reference](api.md) — all assertion methods and decorators
- [Migration from pytest](migration.md) — if you're coming from pytest
