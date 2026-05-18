from collections.abc import Callable
from typing import Annotated

from mathlib import add, clamp, divide, multiply
from tryke import Depends, describe, expect, fixture, test


@fixture
def numbers():
    return [1, 2, 3, 4, 5]


@fixture(per="scope")
def config():
    return {"precision": 2, "max_value": 100}


@fixture
def clamped_add(cfg: Annotated[dict, Depends(config)]):
    def _add(a, b):
        return clamp(add(a, b), 0, cfg["max_value"])

    return _add


with describe("arithmetic"):

    @test
    def test_add():
        expect(add(2, 3), name="2 + 3").to_equal(5)
        expect(add(-1, 1), name="-1 + 1").to_equal(0)

    @test
    def test_multiply():
        expect(multiply(3, 4), name="3 * 4").to_equal(12)
        expect(multiply(0, 99), name="0 * 99").to_equal(0)

    with describe("division"):

        @test
        def test_divide():
            expect(divide(10, 2), name="10 / 2").to_equal(5.0)

        @test
        def test_divide_zero():
            try:
                divide(1, 0)
                expect(True, name="should have raised").to_be_falsy()
            except ValueError as e:
                expect(str(e), name="error message").to_equal("division by zero")


@test.cases(
    test.case("low", value=-5, expected=0),
    test.case("in range", value=50, expected=50),
    test.case("high", value=200, expected=100),
)
def test_clamp(value, expected):
    expect(clamp(value, 0, 100), name="clamped value").to_equal(expected)


@test
def uses_number_list(nums: Annotated[list, Depends(numbers)]):
    expect(nums, name="numbers list").to_have_length(5)
    expect(nums, name="contains 3").to_contain(3)
    nums.append(6)
    expect(nums, name="after append").to_have_length(6)


@test
def clamped_addition(do_add: Annotated[Callable, Depends(clamped_add)]):
    expect(do_add(50, 30), name="50 + 30 clamped").to_equal(80)
    expect(do_add(99, 99), name="99 + 99 clamped").to_equal(100)


@test.skip("not implemented yet")
def future_feature():
    pass


@test.todo("pending design review")
def new_api():
    pass
