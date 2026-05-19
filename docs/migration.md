# Migration from pytest

A side-by-side guide for moving from pytest to Tryke.

!!! tip "Let an LLM do the migration for you"
    The fastest way to migrate is to hand the job to your AI coding
    assistant (Claude Code, Cursor, Codex, Aider, etc.). Install the
    [**pytest-to-tryke-migration skill**](skills/pytest-to-tryke-migration.md)
    — it converts one test file at a time and verifies its discovery
    and outcomes match what pytest produced. The skill page also has a
    Codex `/goal` template that wraps the skill for a whole-repo
    migration.
    **[Install the migration skill &rarr;](skills/pytest-to-tryke-migration.md)**

## Cheat sheet

### Test functions

**pytest:**

```python
def test_addition():
    assert 1 + 1 == 2
```

**Tryke:**

```python
from tryke import expect, test

@test
def addition():
    expect(1 + 1).to_equal(2)
```

The `@test` decorator replaces the `test_` prefix convention. Assertions use `expect()` instead of bare `assert`.

### Assertions

| pytest | Tryke |
|--------|-------|
| `assert x == y` | `expect(x).to_equal(y)` |
| `assert x is y` | `expect(x).to_be(y)` |
| `assert x` | `expect(x).to_be_truthy()` |
| `assert not x` | `expect(x).to_be_falsy()` |
| `assert x is None` | `expect(x).to_be_none()` |
| `assert isinstance(x, cls)` | `expect(x).to_be_instance_of(cls)` |
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

**Tryke:**

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

**Tryke:**

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

**Tryke:**

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

**Tryke** (built-in):

```python
@test
async def async_operation():
    result = await some_coroutine()
    expect(result).to_equal(42)
```

### Filtering

| pytest | Tryke |
|--------|-------|
| `pytest -k "math"` | `tryke test -k "math"` |
| `pytest -m "slow"` | `tryke test -m "slow"` |
| `pytest test_file.py` | `tryke test test_file.py` |
| `pytest test_file.py::test_func` | `tryke test test_file.py` + `-k func` |

### Reporters

| pytest | Tryke |
|--------|-------|
| default verbose | `tryke test` (text reporter) |
| `--tb=short` | `tryke test --reporter dot` |
| `--junit-xml=out.xml` | `tryke test --reporter junit` |
| `--json` (plugin) | `tryke test --reporter json` |

### Running changed tests

**pytest** (requires a plugin like `pytest-picked` or `pytest-testmon`):

```bash
pytest --picked          # from pytest-picked: tests in git-changed files
pytest --testmon         # from pytest-testmon: tests affected at runtime
```

**Tryke** (built-in, uses a static import graph):

```bash
tryke test --changed        # tests affected by git-changed files
tryke test --changed-first  # changed tests first, then the rest
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

### Display names for tests and expectations

Tryke surfaces human-readable labels in reporters where pytest only shows function names and source snippets. Take advantage of this while migrating — names typed once read every time a report is rendered:

```python
@test("returns the cached row on the second call")
def hits_cache():
    expect(rows[0], "first row id").to_equal(42)
    expect(rows[0], "first row name").to_equal("alice")
```

`@test("...")` (or `@test(name="...")`) sets the test's display name; the second positional argument to `expect(value, "...")` labels the assertion. Both are static-only metadata — discovery extracts them at parse time, and they show up in `--reporter llm`, `--reporter junit`, and the default text reporter without any runtime cost.

### Fixtures → `@fixture` + `Depends()`

pytest uses `@pytest.fixture` with implicit parameter-name matching. Tryke uses a single `@fixture` decorator with explicit `Depends()` wiring:

**pytest:**

```python
import pytest

@pytest.fixture(scope="module")
def db():
    conn = create_connection()
    yield conn
    conn.close()

@pytest.fixture
def table(db):
    db.execute("DELETE FROM users")
    return db.table("users")

def test_query(table):
    table.insert({"name": "alice"})
    assert table.count() == 1
```

**Tryke:**

```python
from typing import Annotated

from tryke import test, expect, fixture, Depends

@fixture(per="scope")
def db() -> Connection:
    return create_connection()

@fixture
def managed_conn(conn: Annotated[Connection, Depends(db)]):
    yield conn
    conn.execute("DELETE FROM users")

@test
def query(conn: Annotated[Connection, Depends(managed_conn)]):
    conn.execute("INSERT INTO users (name) VALUES ('alice')")
    expect(conn.execute("SELECT count(*) FROM users")).to_equal(1)
```

Key differences:

- Scope is lexical (where the fixture is defined), not declared via `scope=`
- Dependencies are explicit via `Depends()`, not matched by parameter name
- `Depends()` is fully typed — type checkers see the correct return type
- No `conftest.py` — fixtures live in the same file as the tests they serve

### Parametrize → `@test.cases`

**pytest:**

```python
import pytest

@pytest.mark.parametrize("n,expected", [(0, 0), (1, 1), (10, 100)])
def test_square(n, expected):
    assert n * n == expected
```

**Tryke:**

```python
@test.cases(
    test.case("zero", n=0, expected=0),
    test.case("one",  n=1, expected=1),
    test.case("ten",  n=10, expected=100),
)
def square(n: int, expected: int):
    expect(n * n).to_equal(expected)
```

Labels are arbitrary strings — `"my test"`, `"2 + 3"`, `"negative one"` all work and survive `-k` filtering end-to-end. Case kwargs are statically checked against the function signature under `mypy` / `pyright`.

Each case collects as its own test ID (`fn[label]`), composes with `describe()` blocks, `@fixture`/`Depends()`, and `@test.skip`/`xfail`. See [cases](concepts/cases.md) for the full reference.

#### Runner parametrize (`[asyncio, trio]`)

**pytest** — often seen with `pytest-asyncio` / `anyio`:

```python
@pytest.mark.parametrize("runner", [asyncio, trio])
async def test_under_runner(runner):
    ...
```

**Tryke:**

```python
@test.cases(
    test.case("asyncio", runner=asyncio),
    test.case("trio", runner=trio),
)
async def under_runner(runner):
    ...
```

## Migration skill

For driving the migration with an AI coding assistant, install the
[**pytest-to-tryke-migration skill**](skills/pytest-to-tryke-migration.md).
The skill converts **one test file at a time** and verifies that
file's discovery and outcomes match what pytest produced; a Codex
[`/goal`](https://developers.openai.com/codex/use-cases/follow-goals)
template on the skill page wraps it for a whole-repo migration —
baseline capture, file-by-file iteration, and pytest removal all live
in the goal.

!!! note "LLM reporter"
    When a converted test's outcome diverges from the pytest baseline,
    rerun it with `tryke test -k <name> --reporter llm` — the LLM
    reporter is tuned for context windows and gives concise, structured
    failure diagnostics. See the [reporters guide](guides/reporters.md#llm).
