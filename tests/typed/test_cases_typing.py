"""Type-check sanity cases for ``@test.cases`` + ``test.case`` typing.

The functions in this file are real, runnable tryke tests that also
serve as positive samples for the ``test.case(...)`` typed form: they
must continue to type-check under ``mypy`` and ``pyright``. ty (as of
0.0.21) does not yet enforce the PEP 612 ``Generic[P]`` constructor
pattern ``_CaseSpec`` uses, but the positives below pass under ty
regardless.

**Manual negative verification.** The following mutations of the
positive cases should all be rejected by mypy and pyright (ty currently
accepts them silently until its PEP 612 support catches up). To verify,
introduce one mutation at a time in this file and run
``uvx mypy tests/typed/test_cases_typing.py``:

1. **Unknown kwarg** — change ``test.case("zero", n=0, expected=0)`` to
   ``test.case("zero", n=0, expcted=0)``. Expected: mypy rejects the
   call because ``expcted`` is not in the decorated function's
   signature.

2. **Wrong value type** — change ``n=0`` to ``n="zero"`` in any typed
   case. Expected: mypy rejects because ``n: int`` on ``square``
   doesn't match ``str``.

3. **Missing required kwarg** — drop ``expected=0`` from any typed
   case. Expected: mypy rejects because ``expected`` is a required
   parameter.

4. **Signature drift** — change ``def square(n: int, ...)`` to
   ``def square(n: str, ...)`` without updating the cases. Expected:
   mypy rejects the decoration because ``_P`` stops unifying.

Each mutation must be reverted before committing.
"""

from __future__ import annotations

from tryke import expect, test


@test.cases(
    test.case("zero", n=0, expected=0),
    test.case("one", n=1, expected=1),
    test.case("ten", n=10, expected=100),
)
def square(n: int, expected: int) -> None:
    expect(n * n, "n squared matches expected").to_equal(expected)


@test.cases(
    test.case("my test", name="hello", upper="HELLO"),
    test.case("2 + 3", name="world", upper="WORLD"),
)
def upper(name: str, upper: str) -> None:
    expect(name.upper(), "uppercased name matches").to_equal(upper)


@test("basic").cases(
    test.case("1 + 1", a=1, b=1, expected=2),
    test.case("1 + 2", a=1, b=2, expected=3),
    test.case("1 + 3", a=1, b=3, expected=4),
)
def labelled_addition(a: int, b: int, expected: int) -> None:
    expect(a + b, "a + b matches expected").to_equal(expected)
