"""Collection formats.

>>> 2 + 2
4
"""

from __future__ import annotations

import asyncio
from typing import Annotated

import tryke as tk
import tryke_guard
from tryke import (
    Depends,
    describe,
    expect,
    fixture,
    test,
)
from tryke import (
    describe as suite,
)
from tryke import (
    test as check,
)
from tryke_guard import __TRYKE_TESTING__


@fixture(per="scope")
def scope_value() -> str:
    return "ready"


@fixture
def dependent_value(value: str = Depends(scope_value)) -> int:
    return len(value)


@test
def test_addition() -> None:
    expect(1 + 1).to_equal(2)


@test("Readable subtraction")
def test_subtraction() -> None:
    expect(3 - 1).to_equal(2)


@test()
def test_call_form() -> None:
    pass


@test(name="Keyword display name", tags=["unit", "metadata"])
def test_keyword_metadata() -> None:
    pass


@test
def test_docstring_display_name() -> None:
    """Display name from a docstring."""


@check("Aliased decorator")
def test_symbol_alias() -> None:
    pass


@tk.test(name="Qualified decorator")
def test_qualified_decorator() -> None:
    pass


@test
async def test_async_function() -> None:
    await asyncio.sleep(0)


@test
def test_default_dependency(value: int = Depends(dependent_value)) -> None:
    expect(value).to_equal(5)


@test
def test_annotated_dependency(
    value: Annotated[str, Depends(scope_value)],
) -> None:
    expect(value).to_equal("ready")


@test(name="Rich assertion shapes")
def test_assertion_shapes() -> None:
    value = 3
    expect(value, "non-null value").not_.to_be_none()
    expect(
        expr=value,
        name="multiline equality",
    ).to_equal(
        other=3,
    )
    expect([1, 2, 3]).to_contain(2).fatal()
    expect(lambda: int("not-an-int")).to_raise(ValueError)


@test("Typed cases").cases(
    test.case("one plus one", left=1, right=1, expected=2),
    test.case("known bug", left=1, right=2, expected=4, xfail="Issue #42"),
    test.case("temporarily disabled", left=2, right=2, expected=4, skip="Flaky"),
    test.case("not implemented", left=3, right=3, expected=6, todo="Pending"),
)
def test_typed_cases(left: int, right: int, expected: int) -> None:
    expect(left + right).to_equal(expected)


@test.cases(
    zero={"number": 0, "expected": 0},
    one={"number": 1, "expected": 1},
    three={"number": 3, "expected": 9},
)
def test_keyword_cases(number: int, expected: int) -> None:
    expect(number * number).to_equal(expected)


@test.cases(
    [
        ("empty", {"value": "", "expected": 0}),
        ("word", {"value": "tryke", "expected": 5}),
    ]
)
def test_list_cases(value: str, expected: int) -> None:
    expect(value).to_have_length(expected)


@tk.test.cases(
    tk.test.case("lowercase", value="tryke", expected="TRYKE"),
    tk.test.case("mixed case", value="Tryke", expected="TRYKE"),
)
def test_qualified_cases(value: str, expected: str) -> None:
    expect(value.upper()).to_equal(expected)


with describe("Calculator"):
    with describe("addition"):

        @test
        def test_grouped_integers() -> None:
            pass

    with describe("subtraction"):

        @test
        def test_grouped_negative_result() -> None:
            pass


with suite("Aliased group"):

    @check
    def test_aliased_group() -> None:
        pass


with tk.describe("Qualified group"):
    with tk.describe("Nested"):

        @tk.test(tags=["qualified"])
        def test_qualified_group() -> None:
            pass


@test.skip("Not supported on Windows")
def test_platform_feature() -> None:
    pass


@test.skip
def test_bare_skip() -> None:
    pass


@test.skip(reason="Administrative only", tags=["admin"])
def test_skip_keyword_metadata() -> None:
    pass


@test.skip_if(condition=False, reason="Runtime condition")
def test_conditional_skip() -> None:
    pass


@test.todo("Implement the new API")
def test_new_api() -> None:
    pass


@test.todo
def test_bare_todo() -> None:
    pass


@test.xfail("Upstream issue")
def test_known_failure() -> None:
    pass


@test.xfail
def test_bare_xfail() -> None:
    pass


@test(tags=["slow", "integration"])
def test_tagged() -> None:
    pass


if __TRYKE_TESTING__:

    @test
    def test_bare_testing_guard() -> None:
        pass


if tryke_guard.__TRYKE_TESTING__:

    @tk.test
    def test_qualified_testing_guard() -> None:
        pass


def documented_sum(left: int, right: int) -> int:
    """Add two numbers.

    >>> documented_sum(2, 3)
    5
    """
    return left + right


class Calculator:
    """A small calculator.

    >>> Calculator().double(3)
    6
    """

    factor = 2

    def double(self, value: int) -> int:
        """Double a value.

        >>> Calculator().double(4)
        8
        """
        return self.factor * value
