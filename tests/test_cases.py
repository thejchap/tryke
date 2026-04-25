from __future__ import annotations

from tryke import CasesMarked, Depends, describe, expect, fixture, test
from tryke.expect import _build_cases_table
from tryke.hooks import HookExecutor
from tryke.hooks import fixture as _fixture

with describe("test.cases typed form (test.case)"):

    @test.cases(
        test.case("zero", n=0, expected=0),
        test.case("one", n=1, expected=1),
        test.case("ten", n=10, expected=100),
        test.case("my test", n=2, expected=4),
        test.case("2 + 3", n=5, expected=25),
    )
    def square_typed(n: int, expected: int) -> None:
        expect(n * n, "n squared matches expected").to_equal(expected)


with describe("test.cases kwargs form"):

    @test.cases(
        zero={"n": 0, "squared": 0},
        one={"n": 1, "squared": 1},
        two={"n": 2, "squared": 4},
        ten={"n": 10, "squared": 100},
    )
    def square(n: int, squared: int) -> None:
        expect(n * n, "n squared matches expected").to_equal(squared)

    @test.cases(
        lowercase={"value": "hi", "upper": "HI"},
        already_upper={"value": "HI", "upper": "HI"},
        mixed={"value": "Hi", "upper": "HI"},
    )
    def upper_case(value: str, upper: str) -> None:
        expect(value.upper(), "uppercased value matches").to_equal(upper)


with describe("test.cases list form"):

    @test.cases(
        [
            ("2 + 3", {"a": 2, "b": 3, "total": 5}),
            ("-1 + 1", {"a": -1, "b": 1, "total": 0}),
            ("0 + 0", {"a": 0, "b": 0, "total": 0}),
        ]
    )
    def add(a: int, b: int, total: int) -> None:
        expect(a + b, "a + b matches total").to_equal(total)


with describe("test.cases decorator returns function-like object"):

    @test
    def cases_stamps_attribute() -> None:
        @test.cases(a={"x": 1}, b={"x": 2})
        def fn(x: int) -> None:  # noqa: ARG001 - body never runs, stamping only
            return

        if not isinstance(fn, CasesMarked):
            msg = "decorator should produce a CasesMarked function"
            raise TypeError(msg)
        table = fn.__tryke_cases__
        expect(isinstance(table, tuple), "cases table is a tuple").to_be_truthy()
        expect([e.label for e in table], "case labels in order").to_equal(["a", "b"])
        expect(table[0].kwargs, "first case kwargs").to_equal({"x": 1})
        expect(table[1].kwargs, "second case kwargs").to_equal({"x": 2})
        expect(table[0].args, "first case has no positional args").to_equal(())

    @test
    def cases_list_form_stamps_attribute() -> None:
        @test.cases([("first", {"x": 1}), ("second", {"x": 2})])
        def fn(x: int) -> None:  # noqa: ARG001 - body never runs, stamping only
            return

        if not isinstance(fn, CasesMarked):
            msg = "decorator should produce a CasesMarked function"
            raise TypeError(msg)
        expect(
            [e.label for e in fn.__tryke_cases__], "list-form case labels in order"
        ).to_equal(["first", "second"])

    @test
    def cases_typed_form_stamps_attribute() -> None:
        @test.cases(
            test.case("my test", n=0, expected=0),
            test.case("other test", n=1, expected=1),
        )
        def fn(n: int, expected: int) -> None:  # noqa: ARG001
            return

        if not isinstance(fn, CasesMarked):
            msg = "decorator should produce a CasesMarked function"
            raise TypeError(msg)
        table = fn.__tryke_cases__
        expect([e.label for e in table], "typed-form case labels in order").to_equal(
            ["my test", "other test"]
        )
        expect(table[0].kwargs, "first typed case kwargs").to_equal(
            {"n": 0, "expected": 0}
        )
        expect(table[1].kwargs, "second typed case kwargs").to_equal(
            {"n": 1, "expected": 1}
        )

    @test
    def cases_rejects_mixed_forms() -> None:
        # ty correctly rejects the overload combination at call sites,
        # so we exercise the runtime dispatcher directly to verify the
        # complementary runtime check still raises.
        def attempt() -> None:
            _build_cases_table(([("a", {})],), {"b": {}})

        expect(attempt, "mixing positional and kwargs forms raises").to_raise(
            TypeError, match="positional or kwargs"
        )

    @test
    def cases_rejects_no_args() -> None:
        def attempt() -> None:
            @test.cases()
            def _fn() -> None:
                pass

        expect(attempt, "empty @test.cases() raises").to_raise(TypeError)

    @test
    def cases_rejects_duplicate_labels_typed_form() -> None:
        def attempt() -> None:
            @test.cases(
                test.case("same", n=0, expected=0),
                test.case("same", n=1, expected=1),
            )
            def _fn(n: int, expected: int) -> None:  # noqa: ARG001
                return

        expect(attempt, "duplicate case labels raise").to_raise(
            TypeError, match="duplicate case label 'same'"
        )

    @test
    def cases_rejects_inconsistent_key_sets() -> None:
        def attempt() -> None:
            @test.cases(
                test.case("a", n=0, expected=0),
                test.case("b", n=1),
            )
            def _fn(n: int, expected: int = 0) -> None:  # noqa: ARG001
                return

        expect(attempt, "inconsistent case key sets raise").to_raise(
            TypeError, match="missing"
        )


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
        expect(n * factor, "n scaled by fixture factor").to_equal(expected)


with describe("test.cases composes with describe groups"), describe("nested"):

    @test.cases(a={"x": 1}, b={"x": 2})
    def nested_case(x: int) -> None:
        expect(x, "x is positive").to_be_greater_than(0)


with describe("test.cases composes with modifiers"):

    @test.skip("intentionally skipped — all cases")
    @test.cases(a={"x": 1}, b={"x": 2})
    def skipped_cases(x: int) -> None:  # noqa: ARG001 - body never runs, skip marks it
        msg = "should not run"
        raise AssertionError(msg)

    @test.xfail("known failure — all cases")
    @test.cases(a={"x": 1}, b={"x": 2})
    def xfail_cases(x: int) -> None:
        expect(x, "x equals -1 (expected to fail)").to_equal(-1)


with describe("test.cases rejects kwarg / fixture collision"):

    @test
    def collision_with_fixture_param_raises() -> None:
        @_fixture
        def v() -> int:
            return 42

        def conflict(v: int = Depends(v)) -> None:
            expect(v, "fixture value equals 1").to_equal(1)

        executor = HookExecutor()

        def invoke() -> None:
            executor.run_test(conflict, groups=[], case_kwargs={"v": 1})

        expect(invoke, "case kwarg colliding with fixture raises").to_raise(
            TypeError, match="collide"
        )


with describe("per-case modifiers"):

    @test.cases(
        test.case("normal", n=1, expected=1),
        test.case("skipped", n=2, expected=999, skip="known bug"),
    )
    def per_case_skip(n: int, expected: int) -> None:
        expect(n * n, "n squared matches expected").to_equal(expected)

    @test.cases(
        test.case("passing", n=3, expected=9),
        test.case("xfailing", n=4, expected=-1, xfail="known issue"),
    )
    def per_case_xfail(n: int, expected: int) -> None:
        expect(n * n, "n squared matches expected").to_equal(expected)

    @test.cases(
        test.case("done", n=5, expected=25),
        test.case("placeholder", n=6, expected=0, todo="not implemented yet"),
    )
    def per_case_todo(n: int, expected: int) -> None:
        expect(n * n, "n squared matches expected").to_equal(expected)

    @test
    def case_entry_stores_modifiers() -> None:
        """test.case() steals skip/xfail/todo before forwarding to kwargs."""
        spec = test.case("lbl", x=1, skip="reason", xfail="xr", todo="td")
        entry = spec.entry
        expect(entry.skip, "skip modifier captured").to_equal("reason")
        expect(entry.xfail, "xfail modifier captured").to_equal("xr")
        expect(entry.todo, "todo modifier captured").to_equal("td")
        expect(entry.kwargs, "modifiers removed from kwargs").to_equal({"x": 1})

    @test
    def case_entry_modifiers_default_none() -> None:
        spec = test.case("lbl", x=1)
        entry = spec.entry
        expect(entry.skip, "skip defaults to None").to_be(None)
        expect(entry.xfail, "xfail defaults to None").to_be(None)
        expect(entry.todo, "todo defaults to None").to_be(None)

    @test
    def case_modifier_must_be_string() -> None:
        def build() -> None:
            test.case("bad", skip=42)  # type: ignore[arg-type]

        expect(build, "non-string skip= raises").to_raise(
            TypeError, match="skip= must be a string"
        )

    @test
    def reserved_names_rejected_in_list_form() -> None:
        raw = [("label", {"skip": "oops", "x": 1})]

        expect(
            lambda: _build_cases_table((raw,), {}),
            "reserved kwarg in list-form raises",
        ).to_raise(TypeError, match="reserved name")

    @test
    def reserved_names_rejected_in_kwargs_form() -> None:
        expect(
            lambda: _build_cases_table((), {"todo": {"skip": "oops"}}),
            "reserved kwarg in kwargs-form raises",
        ).to_raise(TypeError, match="reserved name")
