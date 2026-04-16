## Parametrized tests with `@test.cases`

`@test.cases` expands a single test function into one test per input row, so you can exercise the same assertions against a table of inputs without copy-pasting (or sneaking a `for`-loop into the module body where tryke's static discovery can't see it).

Each generated case is a first-class test: it has its own ID (`path::fn[label]`), its own row in reporters, and can be filtered, skipped, or re-run independently.

## Three forms

### Typed form — `test.case(...)` specs (recommended)

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

### Legacy kwargs form — identifier labels

Each keyword is a case label; the value is a dict of arguments passed to the function. This form loses static case-value typing:

```python
@test.cases(
    zero={"n": 0, "expected": 0},
    one={"n": 1, "expected": 1},
    ten={"n": 10, "expected": 100},
)
def square(n: int, expected: int):
    expect(n * n).to_equal(expected)
```

### Legacy list form — arbitrary string labels

For labels that aren't valid Python identifiers. Also loses static case-value typing:

```python
@test.cases([
    ("2 + 3", {"a": 2, "b": 3, "sum": 5}),
    ("-1 + 1", {"a": -1, "b": 1, "sum": 0}),
    ("0 + 0", {"a": 0, "b": 0, "sum": 0}),
])
def add(a: int, b: int, sum: int):
    expect(a + b).to_equal(sum)
```

Prefer the typed form for new code. The legacy forms will continue to work for now but are no longer the recommended API.

## Composition rules

### With `describe()` groups

Cases inherit the group of their enclosing `describe()` block. All cases share the same groups:

```python
with describe("arithmetic"):
    @test.cases(
        small={"x": 1, "doubled": 2},
        big={"x": 100, "doubled": 200},
    )
    def double(x: int, doubled: int):
        expect(x * 2).to_equal(doubled)
```

### With `@fixture` and `Depends()`

Case kwargs and fixture-injected kwargs are merged at call time. Declare fixture params with `Depends()` defaults alongside case params:

```python
from tryke import Depends, expect, fixture, test

@fixture
def multiplier() -> int:
    return 10

@test.cases(
    small={"n": 1, "expected": 10},
    big={"n": 9, "expected": 90},
)
def scaled(n: int, expected: int, factor: int = Depends(multiplier)):
    expect(n * factor).to_equal(expected)
```

A case kwarg must not collide with a fixture-injected parameter — tryke raises `TypeError` at call time if it does. Pick a different name for the case argument.

### With `@test.skip` and `@test.xfail`

Modifiers apply to every generated case:

```python
@test.skip("not ready yet")
@test.cases(a={"x": 1}, b={"x": 2})
def pending(x: int):
    ...

@test.xfail("upstream bug #42")
@test.cases(a={"x": 1}, b={"x": 2})
def known_broken(x: int):
    expect(x).to_equal(-1)
```

Both cases are skipped (or marked xfail) — there's currently no way to mark individual cases. If you need per-case xfail, split the cases into separate functions.

### With `@test`

`@test` and `@test.cases` are mutually exclusive on the same function. Discovery raises an error if both are present. Use `@test.cases` alone — the framework treats it as its own registration decorator.

## The static-analysis constraint

Tryke discovers tests by walking the AST without importing your code. That means `@test.cases(...)` must be **literal**: labels are string or identifier literals, and the top-level shape (typed specs, kwargs, or list of tuples) must be visible in the source.

| Allowed | Rejected |
|---------|----------|
| `@test.cases(test.case("a", n=1))` | `@test.cases(*specs)` |
| `@test.cases(a={"n": 1})` | `@test.cases(build_cases())` |
| `@test.cases([("a", {"n": 1})])` | `@test.cases([*generated])` |
| `@test.cases(test.case("big", n=10**6))` — kwarg values can be expressions | `@test.cases(**labels)` |

For the typed form specifically: the label passed to `test.case(...)` must be a string literal. Dynamic labels (`test.case(label_var, ...)`) are rejected at discovery time.

Non-literal decorator shapes emit a discovery error and the tests are skipped. This mirrors the same constraint that applies to `describe("name")`.

## Soft assertions and cases

[Soft assertions](soft-assertions.md) apply per-case. Each case runs independently, and a failure inside one case never short-circuits the next — every case runs every assertion and reports every failure.

## Comparison with pytest

| pytest | tryke |
|--------|-------|
| `@pytest.mark.parametrize("x,y", [(1, 2), (3, 4)])` | `@test.cases(test.case("a", x=1, y=2), test.case("b", x=3, y=4))` |
| `@pytest.mark.parametrize("x", [1, 2], ids=["one", "two"])` | `@test.cases(test.case("one", x=1), test.case("two", x=2))` |
| Case ID: `test_fn[one-two]` | Case ID: `fn[one]`, `fn[two]` |
| Parameters match by name positionally | Each case is a dict — names are explicit |

The key difference: tryke cases are discovered statically (no import), so editor plugins, `--collect-only`, and distributed scheduling all see every case as a first-class test up front.
