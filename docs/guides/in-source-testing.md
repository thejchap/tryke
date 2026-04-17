# In-source testing

Write tests directly inside the modules they exercise, with zero runtime cost
when the module is imported in production.

## The pattern

Import the guard flag from `tryke_guard`, then put every `@test` (and any
`from tryke import ...` that supports it) inside `if __TRYKE_TESTING__:`.

```python
# myapp/math_utils.py
from tryke_guard import __TRYKE_TESTING__

def add(a: int, b: int) -> int:
    return a + b

if __TRYKE_TESTING__:
    from tryke import test, expect

    @test
    def adds() -> None:
        expect(add(1, 2)).to_equal(3)

    @test
    def adds_negative() -> None:
        expect(add(-1, 1)).to_equal(0)
```

Under `tryke test` the block runs, the decorator registers the test, and
tryke executes it. Outside of tryke — in production, in a REPL, at import
time of the user's app — `__TRYKE_TESTING__` is `False`, CPython skips the
`if` body, and `from tryke import test, expect` never runs.

## Why it's cheap in production

`tryke_guard` is deliberately tiny: the whole module is one env-var read.
Importing it does not pull in the rest of tryke. The body of the `if` block
— including its `from tryke import ...` — is dead code when the flag is
false; the bytecode is emitted but never executed.

If you want to ship production code without tryke installed at all, keep
tryke in your dev dependency group and add `tryke_guard` as the only
runtime-exposed surface. (See `python/tryke_guard.py` — 4 lines of logic,
no third-party dependencies.)

## How the flag is set

The tryke worker flips `tryke_guard.__TRYKE_TESTING__ = True` at startup
before importing any user module. User modules that execute
`from tryke_guard import __TRYKE_TESTING__` afterwards see the new value
bound into their globals, and their `if __TRYKE_TESTING__:` branches run.

Everywhere else — a plain `python` REPL, a production web server, an
ad-hoc script — nothing flips the flag, so it stays `False`.

### Subprocesses default to production mode

Children spawned by your tests start with a fresh `tryke_guard` import. They
do **not** inherit the parent's mutated module attribute, and the
`TRYKE_TESTING` env var is not set by default. So:

```python
import subprocess, sys

@test
def app_starts_cleanly() -> None:
    # Child sees __TRYKE_TESTING__ == False — production mode.
    subprocess.check_call([sys.executable, "-m", "myapp.server", "--check"])
```

If you explicitly want a child in test mode, set the env var:

```python
import os, subprocess, sys

@test
def app_runs_its_own_in_source_tests() -> None:
    subprocess.check_call(
        [sys.executable, "-m", "myapp.server", "--check"],
        env={**os.environ, "TRYKE_TESTING": "1"},
    )
```

`multiprocessing.Process` with `start_method="spawn"` follows the same
rule; `start_method="fork"` inherits the parent's memory, so forked workers
stay in test mode (same process semantics).

## Discovery details

Tryke's static discovery treats `if __TRYKE_TESTING__:` (and the attribute
form `if tryke_guard.__TRYKE_TESTING__:`) as a first-class block — the
same way it handles `with describe("..."):`. Inside a guard you can use:

- `@test`, `@test.cases`, `@test.skip`, `@test.xfail`, `@test.todo`
- `@fixture` (both `per="test"` and `per="scope"`)
- `with describe("..."):` blocks
- Doctests on functions and classes defined inside the guard
- Any static `from X import Y` — imports are followed for `--changed`
  precision

Imports nested inside the guard still contribute to the
[static import graph](../concepts/discovery.md), so changing a helper
module used only by in-source tests still re-selects those tests under
`--changed`.

Dynamic imports (`importlib.import_module(...)`, `__import__(...)`) inside
the guard are **not** treated as always-dirty: they're unreachable in
production, so they don't force the file to re-run on every `--changed`
invocation.

## v1 limitations

To keep discovery tight and errors obvious, v1 recognises only the exact
shapes above. The following are explicitly **not** supported:

| Shape | Behaviour |
|---|---|
| `if __TRYKE_TESTING__: ... else: ...` | Guard is ignored; tryke emits a warning. |
| `if __TRYKE_TESTING__: ... elif ...:` | Guard is ignored; tryke emits a warning. |
| `if not __TRYKE_TESTING__:` | Not recognised as a guard. |
| `if __TRYKE_TESTING__ and other_flag:` | Not recognised as a guard. |
| `if guard_alias:` with `guard_alias = __TRYKE_TESTING__` | Not recognised. |

If you need production fallback code, write it **above or below** the
guard block rather than in an `else` branch:

```python
if __TRYKE_TESTING__:
    from tryke import test, expect
    @test
    def my_test() -> None: ...

# Production fallback, always runs:
DEFAULT_BACKEND = "prod"
```

When a guard with an `else` branch is found, tryke prints a warning like:

```text
warning: myapp/foo.py:12 — `if __TRYKE_TESTING__:` has elif/else; tests
         inside will NOT be discovered. Move production fallback code above
         or below the guard.
```

## Mixing in-source and traditional test layouts

In-source tests and traditional `tests/test_*.py` files coexist freely.
Tryke discovers both in the same run; reporters show them with their file
paths so the source of each test is always clear. You can migrate one
module at a time.

## Editor integrations

The in-source pattern works unchanged in both
[neotest-tryke](https://github.com/thejchap/neotest-tryke) (Neovim) and
[tryke-vscode](https://github.com/thejchap/tryke-vscode) (VS Code) — they
consume tryke's discovery output, which already carries guard-nested tests
with correct file and line information.
