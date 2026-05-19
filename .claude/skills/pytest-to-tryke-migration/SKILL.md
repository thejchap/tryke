---
name: pytest-to-tryke-migration
description: Convert a single pytest test file (or small package) to Tryke and verify the converted file's outcomes match what pytest produced.
---

# pytest → Tryke (per-file)

Convert one test file (or a small package — a directory whose tests
import a shared `conftest.py`) from pytest to
[Tryke](https://tryke.dev). The conversion cheat sheet lives at
<https://tryke.dev/migration.html> and has the full matcher table,
fixture rewrite, and `@test.cases` recipe.

This skill is the **unit of work**, not the whole migration. Repo-level
concerns — capturing a pytest baseline, installing tryke, running
discovery and results parity across the whole suite, removing pytest
from dev deps — belong in a `/goal` that invokes this skill once per
file. See the example goal at the bottom.

## When to use

Activate when you are pointed at a specific pytest test file (or a
small directory of them) and asked to convert it to tryke. The repo
must already have `tryke` installed and a `[tool.tryke]` section in
`pyproject.toml`; if it doesn't, stop and ask — installing is one
line but it's a repo-level decision.

## Steps

### Step 1: Read the file and identify pytest surface

Skim for: `def test_*`, `@pytest.fixture`, `@pytest.mark.parametrize`,
`@pytest.mark.skip(if)?`, `@pytest.mark.xfail`, `@pytest.mark.asyncio`,
`pytest.raises`, `conftest.py` imports, `request.param`,
autouse / scope= kwargs, dynamic `importlib` in the module.

Note anything you don't recognize — flag it before converting rather
than guessing.

### Step 2: Apply conversions

Mechanical rewrites, see the cheat sheet for the full table:

- `def test_foo()` → `@test\ndef foo():` (strip the `test_` prefix —
  it is now redundant)
- `assert x == y` → `expect(x).to_equal(y)` (and the rest of the
  matcher table)
- `@pytest.mark.parametrize(...)` → `@test.cases(test.case("label",
  ...), ...)`
- `@pytest.mark.skip` / `skipif` / `xfail` → `@test.skip(...)` /
  `skip_if(...)` / `xfail(...)`
- `@pytest.mark.asyncio` → drop it; `async def` under `@test` is
  built-in
- `with pytest.raises(E, match=r"..."):` →
  `expect(lambda: ...).to_raise(E, match=r"...")` — **copy the regex
  verbatim**
- `@pytest.fixture` with implicit-name DI → `@fixture` + parameters
  typed as `Annotated[T, Depends(other)]`. Scope is lexical; drop
  `scope=`. Move the fixture into the test module and delete the
  `conftest.py` entry.

While you're in the file, lift docstrings or short phrases into
`@test("...")` display names and label assertions with `expect(value,
"...")` — they cost nothing at runtime and show up in every reporter.

Tryke assertions are **soft by default**: every `expect()` in a test
runs even if an earlier one fails. Only add `.fatal()` when a later
assertion genuinely depends on the earlier one (e.g. you checked
`response.status == 200` and the next assertions dereference the
body). See <https://tryke.dev/concepts/soft-assertions.html>.

### Step 3: Verify

Run the converted file:

```bash
tryke test path/to/file.py
```

Then verify three things:

1. **Discovery.** The set of test IDs in `tryke test --collect-only
   path/to/file.py` matches the original `pytest --collect-only -q
   path/to/file.py`, modulo the `test_` prefix strip and any
   `describe()` group prefixes you added. A missing test usually
   means: fixture still living in a `conftest.py`, a `@test.cases`
   label that isn't a string literal, a plain `def test_foo` that
   never got `@test`, or a test nested inside `if`/`for`/`while`.
2. **Outcomes.** Each test's pass/fail/skip/xfail status matches what
   pytest produced for the same file. If a test that passed under
   pytest now fails, rerun the single test with the LLM-friendly
   reporter and diagnose:

   ```bash
   tryke test -k <name> --reporter llm
   ```

3. **No leftover pytest in this file.** `grep -E
   'pytest|@pytest\.' path/to/file.py` returns nothing.

## Don't

- Don't use `cast()`, `# type: ignore`, `getattr`, or `Any` to
  silence the type checker on `Depends()`. Fix the fixture's return
  type.
- Don't rewrite a `pytest.raises(match=r"...")` regex when moving to
  `to_raise(match=...)`. Pass it through unchanged.
- Don't reflexively add `.fatal()` to every assertion to mimic
  pytest. Diagnose each soft-assertion cascade individually.
- Don't keep a `conftest.py` "just in case" after re-homing its
  fixtures. Delete the empty file.
- Don't paper over a discovery or outcome miss by editing the pytest
  baseline. The baseline is the source of truth.

## Common divergences (Step 3 troubleshooting)

In order of likelihood:

1. Wrong assertion matcher (`to_be` vs `to_equal`, `to_contain` vs
   `to_have_length`, forgotten `.not_`).
2. A fixture's teardown ran at a different scope than pytest's.
3. A soft-assertion cascade: an assertion that would have
   short-circuited under pytest now runs and fails on a `None` the
   earlier assertion was supposed to guard. Add `.fatal()` on the
   guarding assertion.
4. `pytest.raises(match=...)` regex was rewritten instead of copied.
5. Dynamic imports (`importlib.import_module`) in the module or a
   transitive import hide tests from static discovery. Replace with
   static imports.

## Using with `/goal` for a whole-repo migration

Drive the repo-level migration from a Codex `/goal` that invokes this
skill once per file. The goal owns baseline capture, iteration, parity
gates, and cleanup; the skill owns the mechanical conversion of one
file.

```text
/goal Migrate this repository from pytest to tryke. Use the
pytest-to-tryke-migration skill once per test file.

Before starting:
- Capture baseline: `pytest --collect-only -q > .migration/baseline-collect.txt`
  and `pytest --junit-xml=.migration/baseline-results.xml`. Commit
  `.migration/` to .gitignore.
- Install tryke as a dev dep and add a `[tool.tryke]` section
  mirroring the pytest testpaths.

Iterate: for each test file under the configured testpaths, invoke the
skill, then commit and push. Use sub-agents to parallelize batches if
the suite is large.

Done when:
- `tryke test --collect-only` matches the baseline 1:1, modulo the
  `test_` prefix strip and any added describe() group prefixes.
- `tryke test --reporter junit` per-test outcomes match
  .migration/baseline-results.xml after the same normalization.
- pytest and its mechanically-replaced plugins (pytest-asyncio,
  pytest-xdist, pytest-mock) are removed from dev deps.
- CI calls `tryke test` and is green.

Stop and ask if: a file's converted discovery or outcomes diverge in
ways the skill's Step 3 troubleshooting can't explain; a Depends()
typing error would require cast/ignore to silence; or you would need
to mass-add .fatal() to satisfy the outcome parity check.
```
