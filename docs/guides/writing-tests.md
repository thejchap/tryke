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

## Hooks

Hooks run setup and teardown logic around tests. There are six decorators in three pairs:

| Decorator | Runs | Scope |
|-----------|------|-------|
| `@before_each` / `@after_each` | Before/after every test in scope | Per-test |
| `@before_all` / `@after_all` | Once for all tests in scope | Per-scope |
| `@wrap_each` / `@wrap_all` | Generator — yield splits setup/teardown | Per-test / per-scope |

Scope is determined by where the hook is defined: at module top-level it applies to all tests in the file. Inside a `with describe():` block it applies only to tests in that group.

### Basic setup and teardown

```python
from tryke import test, expect, before_each, after_each

@before_each
def setup():
    # Runs before every test in this file
    pass

@after_each
def cleanup():
    # Runs after every test in this file
    pass

@test
def my_test():
    expect(1 + 1).to_equal(2)
```

### Sharing state with `Depends()`

Hooks that produce values share them with tests via `Depends()` in function signatures:

```python
from tryke import test, expect, before_all, before_each, Depends

@before_all
def db() -> Connection:
    return create_connection("test.db")

@before_each
def fresh_table(conn: Connection = Depends(db)) -> Table:
    conn.execute("DELETE FROM users")
    return conn.table("users")

@test
def finds_user(table: Table = Depends(fresh_table)):
    table.insert({"name": "alice"})
    expect(table.count()).to_equal(1)
```

`Depends()` is typed — type checkers see `Depends(db)` as returning `Connection`. At runtime, the framework resolves the dependency chain and passes the values as keyword arguments.

### `before_all` — run once, reuse across tests

`@before_all` hooks run once for their scope. The return value is cached and shared across all tests in that scope via `Depends()`:

```python
@before_all
def db() -> Connection:
    # Called once for the entire file
    return create_connection("test.db")

@test
def test_one(conn: Connection = Depends(db)):
    # Gets the cached connection
    ...

@test
def test_two(conn: Connection = Depends(db)):
    # Same connection instance as test_one
    ...
```

### Wrap hooks — setup and teardown in one function

`@wrap_each` and `@wrap_all` use a generator: code before `yield` is setup, code after is teardown.

```python
from tryke import test, expect, wrap_each, Depends

@wrap_each
def with_transaction(conn: Connection = Depends(db)):
    tx = conn.begin()
    yield tx
    tx.rollback()

@test
def modifies_data(tx: Transaction = Depends(with_transaction)):
    tx.execute("INSERT INTO users (name) VALUES ('alice')")
    expect(tx.query("SELECT count(*) FROM users")).to_equal(1)
    # Transaction rolls back after test — no cleanup needed
```

### Scoping with describe blocks

Hooks defined inside a `describe` block only apply to tests in that block:

```python
from tryke import test, expect, describe, before_all, before_each, Depends

@before_all
def api() -> TestClient:
    return TestClient(app)

with describe("GET /users"):
    @before_each
    def seed_users(client: TestClient = Depends(api)):
        client.post("/users", json={"name": "alice"})

    @test
    def returns_users(client: TestClient = Depends(api)):
        resp = client.get("/users")
        expect(resp.status_code).to_equal(200)

with describe("POST /users"):
    # seed_users does NOT run here — it's scoped to "GET /users"
    @test
    def creates_user(client: TestClient = Depends(api)):
        resp = client.post("/users", json={"name": "bob"})
        expect(resp.status_code).to_equal(201)
```

`api()` runs once for the file (module-level `@before_all`). `seed_users()` runs before each test in "GET /users" only.

### Composing hooks via Depends chains

Hooks can depend on other hooks, forming a dependency graph:

```python
@before_all
def config() -> AppConfig:
    return AppConfig.from_env("test")

@before_all
def db(cfg: AppConfig = Depends(config)) -> Database:
    return Database(cfg.db_url)

@before_all
def cache(cfg: AppConfig = Depends(config)) -> RedisCache:
    return RedisCache(cfg.redis_url)

@before_each
def service(
    db: Database = Depends(db),
    cache: RedisCache = Depends(cache),
) -> UserService:
    return UserService(db, cache)
```

The framework resolves the graph automatically: `config` first (leaf), then `db` and `cache`, then `service`. `@before_all` values are cached for the scope lifetime; `@before_each` values are fresh per test.

### Execution order

For a test inside `describe("users")`:

```text
1. @before_all   (module scope, once for file)
2. @before_each  (module scope, per test)
3. @wrap_each    (module scope, setup half)
4. @before_each  (describe scope, per test)
5. @wrap_each    (describe scope, setup half)
6. TEST RUNS
7. @wrap_each    (describe scope, teardown half)
8. @after_each   (describe scope, reverse order)
9. @wrap_each    (module scope, teardown half)
10. @after_each  (module scope, reverse order)
11. @after_all   (module scope, once after all tests)
```

Outer scope hooks wrap inner scope hooks. Within a scope, `@before_each` and `@wrap_each` run in definition order; `@after_each` runs in reverse (stack unwinding). Teardown always runs, even if the test fails.

### Concurrency impact

See [concurrency](../concepts/concurrency.md#hooks-and-scheduling) for how hooks interact with `--dist` modes. In short: `@before_each` hooks preserve full parallelism, while `@before_all` hooks force tests in the same scope onto a single worker.

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
