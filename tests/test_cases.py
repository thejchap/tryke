from __future__ import annotations

from tryke import CasesMarked, Depends, describe, expect, fixture, test
from tryke.hooks import HookExecutor
from tryke.hooks import fixture as _fixture

with describe("test.cases kwargs form"):

    @test.cases(
        zero={"n": 0, "squared": 0},
        one={"n": 1, "squared": 1},
        two={"n": 2, "squared": 4},
        ten={"n": 10, "squared": 100},
    )
    def square(n: int, squared: int) -> None:
        expect(n * n).to_equal(squared)

    @test.cases(
        lowercase={"value": "hi", "upper": "HI"},
        already_upper={"value": "HI", "upper": "HI"},
        mixed={"value": "Hi", "upper": "HI"},
    )
    def upper_case(value: str, upper: str) -> None:
        expect(value.upper()).to_equal(upper)


with describe("test.cases list form"):

    @test.cases(
        [
            ("2 + 3", {"a": 2, "b": 3, "total": 5}),
            ("-1 + 1", {"a": -1, "b": 1, "total": 0}),
            ("0 + 0", {"a": 0, "b": 0, "total": 0}),
        ]
    )
    def add(a: int, b: int, total: int) -> None:
        expect(a + b).to_equal(total)


with describe("test.cases decorator returns function-like object"):

    @test
    def cases_stamps_attribute() -> None:
        @test.cases(a={"x": 1}, b={"x": 2})
        def fn(x: int) -> None:  # noqa: ARG001 - body never runs, stamping only
            return

        if not isinstance(fn, CasesMarked):
            msg = "decorator should produce a CasesMarked function"
            raise TypeError(msg)
        cases = fn.__tryke_cases__
        expect(isinstance(cases, dict)).to_be_truthy()
        expect(list(cases.keys())).to_equal(["a", "b"])
        expect(cases["a"]).to_equal({"x": 1})
        expect(cases["b"]).to_equal({"x": 2})

    @test
    def cases_list_form_stamps_attribute() -> None:
        @test.cases([("first", {"x": 1}), ("second", {"x": 2})])
        def fn(x: int) -> None:  # noqa: ARG001 - body never runs, stamping only
            return

        if not isinstance(fn, CasesMarked):
            msg = "decorator should produce a CasesMarked function"
            raise TypeError(msg)
        expect(list(fn.__tryke_cases__.keys())).to_equal(["first", "second"])

    @test
    def cases_rejects_mixed_forms() -> None:
        def attempt() -> None:
            @test.cases([("a", {})], b={})
            def _fn() -> None:
                pass

        expect(attempt).to_raise(TypeError, match="list form")

    @test
    def cases_rejects_no_args() -> None:
        def attempt() -> None:
            @test.cases()
            def _fn() -> None:
                pass

        expect(attempt).to_raise(TypeError)


with describe("test.cases composes with fixtures"):

    @fixture
    def multiplier() -> int:
        return 10

    @test.cases(
        small={"n": 1, "expected": 10},
        medium={"n": 5, "expected": 50},
        big={"n": 9, "expected": 90},
    )
    def scaled(n: int, expected: int, factor: int = Depends(multiplier)) -> None:
        expect(n * factor).to_equal(expected)


with describe("test.cases composes with describe groups"), describe("nested"):

    @test.cases(a={"x": 1}, b={"x": 2})
    def nested_case(x: int) -> None:
        expect(x).to_be_greater_than(0)


with describe("test.cases composes with modifiers"):

    @test.skip("intentionally skipped — all cases")
    @test.cases(a={"x": 1}, b={"x": 2})
    def skipped_cases(x: int) -> None:  # noqa: ARG001 - body never runs, skip marks it
        msg = "should not run"
        raise AssertionError(msg)

    @test.xfail("known failure — all cases")
    @test.cases(a={"x": 1}, b={"x": 2})
    def xfail_cases(x: int) -> None:
        expect(x).to_equal(-1)


with describe("test.cases rejects kwarg / fixture collision"):

    @test
    def collision_with_fixture_param_raises() -> None:
        @_fixture
        def v() -> int:
            return 42

        def conflict(v: int = Depends(v)) -> None:
            expect(v).to_equal(1)

        executor = HookExecutor()

        def invoke() -> None:
            executor.run_test(conflict, groups=[], case_kwargs={"v": 1})

        expect(invoke).to_raise(TypeError, match="collide")
