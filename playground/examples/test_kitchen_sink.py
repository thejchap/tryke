from typing import Annotated

from mathlib import add, clamp, divide
from tryke import Depends, describe, expect, fixture, test


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
        expect(add(2, 3), "2 + 3").to_equal(5)
        expect(add(-1, 1), "-1 + 1").to_equal(0)

    with describe("division"):

        @test
        def test_divide():
            expect(divide(10, 2), "10 / 2").to_equal(5.0)

        @test
        def test_divide_zero():
            expect(lambda: divide(2, 0), "2 / 0").to_raise(ValueError)


@test("clamp").cases(
    test.case("low", value=-5, expected=0),
    test.case("in range", value=50, expected=50),
    test.case("high", value=200, expected=100),
)
def test_clamp(value: int, expected: int):
    expect(clamp(value, 0, 100), name="clamped value").to_equal(expected)
