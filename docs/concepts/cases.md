## Parametrized tests with `@test.cases`

`@test.cases` expands a single test function into one test per input row, so you can exercise the same assertions against a table of inputs without copy-pasting (or sneaking a `for`-loop into the module body where Tryke's static discovery can't see it).

Each generated case is a first-class test: it has its own ID (`path::fn[label]`), its own row in reporters, and can be filtered, skipped, or re-run independently.

## Declaring cases

Pass one `test.case(label, ...)` spec per row. Labels are arbitrary strings (spaces, math operators, punctuation all fine), and the kwargs you pass are checked against the decorated function's signature by `mypy` / `pyright` via a [PEP 612](https://peps.python.org/pep-0612/) `ParamSpec`:

```python
from tryke import expect, test

@test.cases(
    test.case("zero",    n=0,  expected=0),
    test.case("one",     n=1,  expected=1),
    test.case("my test", n=10, expected=100),
)
def square(n: int, expected: int):
    expect(n * n).to_equal(expected)
```

This collects as three tests: `square[zero]`, `square[one]`, `square[my test]`.

Typing caught statically under `mypy` / `pyright`:

- **Unknown kwarg** — `test.case("bad", n=0, expcted=0)` against `square(n, expected)` — `expcted` isn't in the signature.
- **Wrong value type** — `test.case("bad", n="zero", expected=0)` against `n: int` — `str` doesn't match.
- **Missing required kwarg** — `test.case("bad", n=0)` — the signature requires `expected`.
- **Signature drift** — changing `def square(n: int, ...)` to `def square(n: str, ...)` after cases are written — the cases stop unifying with `_P`.

`ty` (Astral's type checker, as of 0.0.21) does not yet fully enforce the PEP 612 `Generic[P]` constructor pattern this API uses — it will accept the above negatives silently. Runtime validation still catches label collisions and inconsistent key sets across cases regardless of the type checker.

## Composition rules

### With `describe()` groups

Cases inherit the group of their enclosing `describe()` block. All cases share the same groups:

```python
with describe("arithmetic"):
    @test.cases(
        test.case("small", x=1, doubled=2),
        test.case("big", x=100, doubled=200),
    )
    def double(x: int, doubled: int):
        expect(x * 2).to_equal(doubled)
```

### With `@fixture` and `Depends()`

Case kwargs and fixture-injected kwargs are merged at call time. Declare fixture params with `Depends()` alongside case params:

```python
from typing import Annotated

from tryke import Depends, expect, fixture, test

@fixture
def multiplier() -> int:
    return 10

@test.cases(
    test.case("small", n=1, expected=10),
    test.case("big", n=9, expected=90),
)
def scaled(n: int, expected: int, factor: Annotated[int, Depends(multiplier)]):
    expect(n * factor).to_equal(expected)
```

A case kwarg must not collide with a fixture-injected parameter — Tryke raises `TypeError` at call time if it does. Pick a different name for the case argument.

### With `@test.skip`, `@test.xfail`, and `@test.todo`

#### Function-level modifiers

Decorators apply to every generated case:

```python
@test.skip("not ready yet")
@test.cases(
    test.case("a", x=1),
    test.case("b", x=2),
)
def pending(x: int):
    ...
```

Both cases are skipped.

#### Per-case modifiers

Pass `skip`, `xfail`, or `todo` as keyword arguments to `test.case()` to mark individual cases:

```python
@test.cases(
    test.case("normal", n=1, expected=1),
    test.case("broken", n=2, expected=999, xfail="known bug #42"),
    test.case("pending", n=3, expected=9, skip="waiting on upstream"),
    test.case("placeholder", n=4, expected=16, todo="not implemented"),
)
def square(n: int, expected: int):
    expect(n * n).to_equal(expected)
```

`skip`, `xfail`, and `todo` are reserved keyword names in `test.case()` — they are consumed by the framework before the remaining kwargs are forwarded to the test function. They must be strings.

**Precedence:** a per-case modifier overrides the function-level modifier of the same kind. If a case has `skip="reason"`, it uses that reason regardless of `@test.skip` on the function. Cases without a per-case modifier inherit the function-level modifier.

Per-case modifiers are only supported in the `test.case(...)` spec form. The legacy kwargs and list overloads of `@test.cases` (visible in the API reference) raise `TypeError` if `skip`, `xfail`, or `todo` appear as keys — the framework cannot distinguish modifier intent from test data in those forms.

**Static discovery:** for per-case modifiers to be recognized at discovery time (before import), the value must be a string literal. Non-literal values (e.g. `skip=some_variable`) are handled at runtime as a fallback.

### With `@test`

`@test` and `@test.cases` are mutually exclusive on the same function. Discovery raises an error if both are present. Use `@test.cases` alone — the framework treats it as its own registration decorator.

## The static-analysis constraint

Tryke discovers tests by walking the AST without importing your code. That means `@test.cases(...)` must be **literal**: labels are string literals, and the top-level shape must be visible in the source.

| Allowed | Rejected |
|---------|----------|
| `@test.cases(test.case("a", n=1))` | `@test.cases(*specs)` |
| `@test.cases(test.case("big", n=10**6))` — kwarg values can be expressions | `@test.cases(build_cases())` |

Dynamic labels (`test.case(label_var, ...)`) are rejected at discovery time.

Non-literal decorator shapes emit a discovery error and the tests are skipped. This mirrors the same constraint that applies to `describe("name")`.

## Soft assertions and cases

[Soft assertions](soft-assertions.md) apply per-case. Each case runs independently, and a failure inside one case never short-circuits the next — every case runs every assertion and reports every failure.
