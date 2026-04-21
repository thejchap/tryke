# Writing tests

## The `@test` decorator

Mark any function as a test with the `@test` decorator:

```python
from tryke import expect, test

@test
def addition():
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

If you don't pass `name=`, Tryke uses the first line of the function's docstring as the display name, falling back to the function name if there is no docstring:

```python
@test
def addition():
    """1 + 1 should equal 2."""
    expect(1 + 1).to_equal(2)
```

Both `@test(name="...")` and a docstring produce the same output in reporters.

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
| `to_be_instance_of(cls)` | `isinstance(x, cls)` (accepts a tuple of types) |
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

## In-source testing

Put tests directly inside production modules by guarding them with
`__TRYKE_TESTING__` from the tiny `tryke_guard` module:

```python
# myapp/math_utils.py
from tryke_guard import __TRYKE_TESTING__

def add(a: int, b: int) -> int:
    return a + b

if __TRYKE_TESTING__:
    from tryke import test, expect

    @test
    def adds():
        expect(add(1, 2)).to_equal(3)
```

In production `__TRYKE_TESTING__` is `False`, the `if` block is dead code,
and `tryke` itself never loads. Under `tryke test` the worker flips the flag
at startup and Tryke discovers the guarded tests exactly like top-level
ones. `@test`, `@test.cases`, `@fixture`, `with describe(...)`, and
doctests all work inside the guard; imports inside it participate in the
`--changed` import graph.

**Subprocesses default to production mode.** The worker sets the flag via a
module attribute, not an env var, so `subprocess.run([...])` and
`multiprocessing.Process(start_method="spawn")` children start with
`__TRYKE_TESTING__ == False`. Opt a child in with
`env={**os.environ, "TRYKE_TESTING": "1"}`.

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

## Fixtures

Fixtures run setup and teardown logic around tests. There is a single decorator, `@fixture`, with two granularities:

| Form | Runs | Scope |
|------|------|-------|
| `@fixture` (default: `per="test"`) | Before/after every test in scope | Per-test |
| `@fixture(per="scope")` | Once for all tests in scope | Per-(lexical-)scope |

Scope is determined by *where* the fixture is defined: at module top-level it applies to all tests in the file. Inside a `with describe():` block it applies only to tests in that group.

Setup and teardown live in the same function. Use `yield` to split them:

```python
from tryke import fixture, test, expect

@fixture
def db():
    conn = connect("test.db")
    yield conn             # value available to tests via Depends()
    conn.close()           # teardown runs after each test

@fixture
def fresh_user():
    return {"name": "alice"}   # plain return: no teardown
```

### Sharing state with `Depends()`

Fixtures that produce values share them with tests via `Depends()` in function signatures:

```python
from tryke import fixture, test, expect, Depends

@fixture(per="scope")
def db() -> Connection:
    return create_connection("test.db")

@fixture
def fresh_table(conn: Connection = Depends(db)) -> Table:
    conn.execute("DELETE FROM users")
    return conn.table("users")

@test
def finds_user(table: Table = Depends(fresh_table)):
    table.insert({"name": "alice"})
    expect(table.count()).to_equal(1)
```

`Depends()` is typed — type checkers see `Depends(db)` as returning `Connection`. At runtime, the framework resolves the dependency chain and passes the values as keyword arguments.

### `per="scope"` — run once, reuse across tests

`@fixture(per="scope")` fixtures run once for their lexical scope. The return value is cached and shared across all tests in that scope:

```python
@fixture(per="scope")
def db() -> Connection:
    # Called once for the entire file
    return create_connection("test.db")

@test
def first_query(conn: Connection = Depends(db)):
    # Gets the cached connection
    ...

@test
def second_query(conn: Connection = Depends(db)):
    # Same connection instance as first_query
    ...
```

> **Shared by reference.** The value returned from a `per="scope"` fixture is cached once per scope and handed to every test by reference. If a test mutates it, the mutation is visible to subsequent tests on the same worker. Treat `per="scope"` values as read-only unless they represent resources where mutation is part of the contract (connections, temp directories). See [concurrency: same-worker sharing of `per="scope"` values](../concepts/concurrency.md#same-worker-sharing-of-perscope-values) for details.

### Setup + teardown in one function

Use `yield` to express teardown. Code before `yield` is setup; code after is teardown. Works for both `per="test"` and `per="scope"`:

```python
from tryke import fixture, test, expect, Depends

@fixture
def with_transaction(conn: Connection = Depends(db)):
    tx = conn.begin()
    yield tx
    tx.rollback()         # runs after the test

@test
def modifies_data(tx: Transaction = Depends(with_transaction)):
    tx.execute("INSERT INTO users (name) VALUES ('alice')")
    expect(tx.query("SELECT count(*) FROM users")).to_equal(1)
    # Transaction rolls back after test — no cleanup needed
```

### Scoping with describe blocks

Fixtures defined inside a `describe` block only apply to tests in that block:

```python
from tryke import fixture, test, expect, describe, Depends

@fixture(per="scope")
def api() -> TestClient:
    return TestClient(app)

with describe("GET /users"):
    @fixture
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

`api()` runs once for the file (module-level `per="scope"`). `seed_users()` runs before each test in "GET /users" only.

### Composing fixtures via Depends chains

Fixtures can depend on other fixtures, forming a dependency graph:

```python
@fixture(per="scope")
def config() -> AppConfig:
    return AppConfig.from_env("test")

@fixture(per="scope")
def db(cfg: AppConfig = Depends(config)) -> Database:
    return Database(cfg.db_url)

@fixture(per="scope")
def cache(cfg: AppConfig = Depends(config)) -> RedisCache:
    return RedisCache(cfg.redis_url)

@fixture
def service(
    db: Database = Depends(db),
    cache: RedisCache = Depends(cache),
) -> UserService:
    return UserService(db, cache)
```

The framework resolves the graph automatically: `config` first (leaf), then `db` and `cache`, then `service`. `per="scope"` values are cached for the scope lifetime; `per="test"` values are fresh per test.

### Execution order

For a test inside `describe("users")`:

```text
1. per="scope" fixtures (module scope, once for file)
2. per="test"  fixtures (module scope, definition order)
3. per="scope" fixtures (describe scope, once for group)
4. per="test"  fixtures (describe scope, definition order)
5. TEST RUNS
6. per="test"  fixtures teardown (reverse of setup, LIFO)
7. per="scope" fixtures teardown runs only on module/group finalize
```

Outer scope fixtures wrap inner scope fixtures. Within a scope, fixtures set up in definition order and tear down in reverse — exactly like `contextlib.ExitStack`. Teardown always runs, even if the test fails.

### Concurrency impact

See [concurrency](../concepts/concurrency.md#fixtures-and-scheduling) for how fixtures interact with `--dist` modes. In short: `per="test"` fixtures preserve full parallelism, while `per="scope"` fixtures force tests in the same scope onto a single worker.

## Parametrized tests

Use `@test.cases` to run the same test against multiple inputs. Each case collects as a distinct test (`fn[label]`) with its own row in reporters:

```python
@test.cases(
    test.case("zero",    n=0,  expected=0),
    test.case("one",     n=1,  expected=1),
    test.case("my test", n=10, expected=100),
)
def square(n: int, expected: int):
    expect(n * n).to_equal(expected)
```

Labels are arbitrary strings — spaces and operators are fine — and each case's kwargs are checked against the decorated function's signature under `mypy` / `pyright` via a [PEP 612](https://peps.python.org/pep-0612/) `ParamSpec`.

Cases compose with `describe()`, `@fixture` / `Depends()`, and modifiers. See [the cases concept page](../concepts/cases.md) for composition rules and the static-analysis constraint on decorator arguments.

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

If the test passes unexpectedly, Tryke reports it so you know the issue may be resolved.
