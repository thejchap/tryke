# Migration from pytest

A side-by-side guide for moving from pytest to Tryke.

!!! tip "Let an LLM do the migration for you"
    The fastest way to migrate is to hand the job to your AI coding assistant
    (Claude Code, Cursor, Codex, Aider, etc.). We maintain a battle-tested
    prompt that walks the assistant through a **phased, gated** migration with
    explicit discovery- and results-parity checks so nothing is silently
    dropped or inverted — it pairs with `tryke test --reporter llm` for
    concise, structured failure diagnostics tuned to LLM context windows.
    **[Jump to the migration prompt &rarr;](#migration-prompt)**

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

**pytest** (requires plugin):

```bash
pytest --lf  # last failed
```

**Tryke** (built-in):

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
from tryke import test, expect, fixture, Depends

@fixture(per="scope")
def db() -> Connection:
    return create_connection()

@fixture
def managed_conn(conn: Connection = Depends(db)):
    yield conn
    conn.execute("DELETE FROM users")

@test
def query(conn: Connection = Depends(managed_conn)):
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

## Migration prompt

The prompt below is designed to be pasted into an AI coding assistant (Claude Code, Cursor, Aider, etc.) pointed at a repository that already has a working pytest suite. It walks the assistant through a phased migration with explicit stop-and-verify gates: a **discovery-parity** gate after mechanical conversion, then a **results-parity** gate after the first `tryke test` run. Do not skip the gates — most silent migration failures are a test that stopped being collected or an assertion that quietly inverted.

Copy everything inside the fence and hand it to your assistant.

````markdown
# Task: migrate this repository from pytest to Tryke

You are migrating a Python repository from **pytest** to **Tryke**
(<https://tryke.dev>). Tryke is a Rust-based test runner with a Jest-style API
(`@test`, `expect(...).to_equal(...)`, `@fixture` + `Depends()`, `@test.cases`).
The migration cheat sheet is at <https://tryke.dev/migration/>.

Work in the phases below. **Do not advance to the next phase until the stated
verification gate passes.** Stop and ask me if a gate fails and you cannot
trivially explain the mismatch.

Create a `.migration/` directory at the repo root for baseline and comparison
artifacts; add it to `.gitignore`. Keep a running `.migration/NOTES.md` of
decisions and mismatches.

## Phase 0 — Baseline capture (pytest)

On a clean checkout, before any code changes:

1. `pytest --collect-only -q > .migration/pytest-collect.txt 2>&1`
2. `pytest --junit-xml=.migration/pytest-results.xml` (let failures surface
   if any — we just need the XML)
3. Record in `.migration/NOTES.md`:
   - total collected
   - passed / failed / skipped / xfailed / errored counts
   - any collection errors (these **must** be fixed on pytest first — do not
     proceed with a broken baseline)
4. List every active pytest plugin from `pyproject.toml` /
   `requirements*.txt` / `setup.cfg` so we know what behavior we are
   replacing (e.g. `pytest-asyncio`, `pytest-xdist`, `pytest-mock`,
   `pytest-django`).

**Gate 0:** baseline collection has zero errors and results XML exists.

## Phase 1 — Install and configure Tryke

1. Add `tryke` as a dev dependency (`uv add --dev tryke` or the
   project's equivalent). See <https://tryke.dev/guides/installation/>.
2. Add a `[tool.tryke]` section to `pyproject.toml` mirroring the
   pytest `testpaths` / `norecursedirs` values. See
   <https://tryke.dev/guides/configuration/>.
3. Run `tryke --help` and `tryke test --help` to confirm the binary works.

**Gate 1:** `tryke test --collect-only` runs without crashing (it is fine if
it finds **zero** tests at this point — no files have been converted yet).

## Phase 2 — Mechanical conversion

Convert test files using the cheat sheet at
<https://tryke.dev/migration/>. Do one package / directory at a time and keep
each conversion small enough to review.

For each file:

- Replace non-parametrized `def test_foo(...)` with `@test` + `def foo(...)`.
- Replace `assert` with `expect(...).to_...()` per the assertions table.
- Replace `pytest.raises(Exc, match=...)` with
  `expect(lambda: ...).to_raise(Exc, match=...)` — **use the `match=` kwarg
  verbatim**, do not rewrite the regex by eye.
- Replace `@pytest.mark.parametrize` with `@test.cases(test.case("label", ...))`.
  For parametrized tests, use `@test.cases(...)` **instead of** `@test` — the
  two decorators are mutually exclusive on the same function and discovery
  raises an error if both are present. Labels must be string literals (static
  analysis constraint — see <https://tryke.dev/concepts/cases/>). Each case
  kwarg must match the function signature.
- Replace `@pytest.mark.skip` / `skipif` / `xfail` with `@test.skip` /
  `@test.skip_if` / `@test.xfail`.
- Replace `@pytest.mark.asyncio` with plain `async def` under `@test`.
- Replace `@pytest.fixture` with `@fixture`; wire dependencies with
  `Depends(other_fixture)` instead of parameter-name matching. Fixtures are
  lexically scoped — move them into the module that uses them and **delete
  the `conftest.py` entry** rather than leaving both.

Tryke discovery is **static** (Ruff-based AST parse). See
<https://tryke.dev/concepts/discovery/>. This has two consequences:

- `importlib.import_module()` / `__import__()` at module scope will mark the
  file always-dirty and defeat `--changed` mode. Replace with static imports
  or isolate the dynamic logic in non-test code.
- Tryke descends only into `with describe("..."):` and
  `if __TRYKE_TESTING__:` blocks. Tests nested inside `if/for/while/try`
  bodies (uncommon in pytest too) will not be discovered — flatten them.

**Soft assertions — read this carefully.** Tryke assertions are soft by
default: every `expect()` in a test runs even if an earlier one fails. See
<https://tryke.dev/concepts/soft-assertions/>. **Do not** reflexively add
`.fatal()` to every assertion to mimic pytest. Only add `.fatal()` when a
later assertion genuinely depends on the earlier one (e.g. you checked
`response.status == 200` and the next assertions dereference the body).

### Don't

- Do **not** use `cast()`, `# type: ignore`, `getattr`, or `Any` to silence
  the type checker on `Depends()` — fix the fixture's return type instead.
- Do **not** translate `pytest.raises(match=r"...")` by rewriting the regex
  — pass it through unchanged on `to_raise(match=...)`.
- Do **not** keep `conftest.py` files "just in case" after moving the
  fixtures. Delete them.
- Do **not** paper over a missing test in Phase 3 by editing the baseline
  file. The baseline is the source of truth.

## Phase 3 — Discovery parity gate

1. `tryke test --collect-only > .migration/tryke-collect.txt`
2. Normalize both lists and diff them:
   - pytest: `test_file.py::test_name[case_label]`
   - tryke:  `test_file.py::name[case_label]`
   - The only expected differences are the `test_` prefix stripping and the
     describe-group prefixes where you added `with describe(...)` blocks.
3. For each test present in pytest's list but missing from Tryke's,
   diagnose in this order:
   1. Dynamic imports in the module or a transitive import
      (`grep -R importlib.import_module`). Replace with static imports.
   2. Fixture still living in `conftest.py` — move it module-local.
   3. `@test.cases` label that is not a string literal, or case kwargs that
      don't match the function signature.
   4. A plain `def test_foo` that never got decorated with `@test`.
   5. A test nested inside an `if`/`for`/`while` body — flatten.
4. For each test present in Tryke's list but missing from pytest's, you
   probably double-collected a `describe()` group. Fix the decorator.

**Gate 3:** the two collect lists match 1:1 (modulo the documented prefix
changes). Record the final count in `.migration/NOTES.md`.

## Phase 4 — Results parity gate

1. `tryke test --reporter junit > .migration/tryke-results.xml`
2. Compare per-test outcomes against `.migration/pytest-results.xml`. The
   JUnit `<testcase>` names do **not** match byte-for-byte — Phase 2 stripped
   the `test_` prefix and Phase 3 may have added `describe()` group prefixes.
   Apply the same normalization you used to pass Gate 3 before comparing:
   - Strip the leading `test_` from pytest names
     (`test_file.py::test_add` → `test_file.py::add`).
   - For any test you moved under a `with describe("group"):` block, prepend
     `group::` on the pytest side (or strip it on the Tryke side) so the
     IDs line up.
   - Parametrize labels (`[case_label]`) already match between systems — do
     not rewrite them.

   Then for each normalized name, compare pass / fail / skip / xfail status.
3. For each divergence:
   - Rerun the single test with the LLM-friendly reporter:
     `tryke test -k <name> --reporter llm`
     (see <https://tryke.dev/guides/reporters/>). Feed that output back
     through this prompt to diagnose.
   - Common causes, in order of likelihood:
     - Wrong assertion matcher (`to_be` vs `to_equal`, `to_contain` vs
       `to_have_length`, forgotten `.not_`).
     - A fixture's teardown ran at a different scope than pytest's.
     - A soft-assertion cascade: an assertion that would have short-circuited
       under pytest now runs and fails on a `None` the earlier assertion
       was supposed to guard. Add `.fatal()` on the guarding assertion.
     - `pytest.raises(match=...)` regex was rewritten instead of copied.
4. Do not mass-add `.fatal()` to make numbers match — diagnose each case.

**Gate 4:** per-test outcomes match the pytest baseline. Record the final
counts in `.migration/NOTES.md`.

## Phase 5 — Cleanup

1. Remove `pytest`, `pytest-asyncio`, `pytest-xdist`, `pytest-mock` (if the
   usage was mechanical), and other pytest-only plugins from dev
   dependencies. Leave anything whose functionality Tryke does not yet
   provide and flag it in `.migration/NOTES.md`.
2. Delete empty `conftest.py` files.
3. Update CI to call `tryke test --reporter junit` (or `--reporter llm`
   if CI logs are consumed by an LLM) in place of `pytest`.
4. Update the project README's testing section.
5. Delete `.migration/` once you have committed the migration.

**Gate 5:** CI is green on Tryke; pytest is no longer installed.

## Reporting back

After each gate, summarize in chat:

- which files changed in this phase
- the current test counts (collected / passed / failed / skipped / xfailed)
- anything you had to skip or flag for human review
````

!!! note "LLM reporter"
    Phase 4 uses `tryke test --reporter llm` specifically because its output
    is tuned for LLM context windows — concise, structured failure
    diagnostics. See the [reporters guide](guides/reporters.md#llm).
