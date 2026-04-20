from __future__ import annotations

import asyncio
import importlib.util
import json
from pathlib import Path
from tempfile import TemporaryDirectory
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from types import ModuleType

from tryke import describe, expect, test
from tryke.expect import (
    Expectation,
    ExpectationError,
    MatchResult,
    SoftContext,
    _set_soft_context,
    _SkipMarked,
)

with describe("expectations"):

    @test(name="basic equality")
    def test_basic() -> None:
        expect(1).to_equal(1)
        expect("hello").to_equal("hello")

    @test(name="identity with to_be")
    def test_to_be() -> None:
        sentinel = object()
        expect(sentinel).to_be(sentinel)
        expect(None).to_be(None)

    @test(name="to_be_truthy")
    def test_to_be_truthy() -> None:
        expect(1).to_be_truthy()
        expect("x").to_be_truthy()
        expect([1]).to_be_truthy()

    @test(name="to_be_falsy")
    def test_to_be_falsy() -> None:
        expect(0).to_be_falsy()
        expect("").to_be_falsy()
        expect([]).to_be_falsy()

    @test(name="to_be_none")
    def test_to_be_none() -> None:
        expect(None).to_be_none()
        expect(1).not_.to_be_none()

    @test(name="to_be_instance_of")
    def test_to_be_instance_of() -> None:
        expect([1, 2, 3]).to_be_instance_of(list)
        expect("hello").to_be_instance_of(str)
        expect("hi").to_be_instance_of((bytes, str))
        # `bool` is a subclass of `int`, so `type[bool]` is assignable
        # to `type[int]` and this stays type-clean while still failing
        # at runtime (42 is an int, not a bool).
        expect(42).not_.to_be_instance_of(bool)
        expect(1.5).not_.to_be_instance_of((list, dict))

    @test(name="to_be_instance_of narrows subclasses")
    def test_to_be_instance_of_subclass() -> None:
        class Base:
            pass

        class Derived(Base):
            pass

        # Downcast: the static type is Base, so asking "is it a Derived?"
        # is a real runtime question and `type[Derived]` is assignable to
        # the expected `type[Base]` via covariance.
        derived_as_base: Base = Derived()
        expect(derived_as_base).to_be_instance_of(Derived)
        plain_base: Base = Base()
        expect(plain_base).not_.to_be_instance_of(Derived)

    @test(name="to_be_instance_of reports class names on failure")
    def test_to_be_instance_of_error_fields() -> None:
        ctx = SoftContext()
        _set_soft_context(ctx)
        try:
            expect(42).to_be_instance_of(bool).fatal()
        except ExpectationError as exc:
            _set_soft_context(None)
            expect(exc.expected).to_equal("instance of bool")
            expect(exc.received).to_equal("instance of int")
        else:
            _set_soft_context(None)
            msg = "ExpectationError was not raised"
            raise AssertionError(msg)

    @test(name="to_be_instance_of accepts a tuple of classes")
    def test_to_be_instance_of_tuple_error_fields() -> None:
        ctx = SoftContext()
        _set_soft_context(ctx)
        try:
            expect(1.5).to_be_instance_of((list, dict)).fatal()
        except ExpectationError as exc:
            _set_soft_context(None)
            expect(exc.expected).to_equal("instance of list | dict")
            expect(exc.received).to_equal("instance of float")
        else:
            _set_soft_context(None)
            msg = "ExpectationError was not raised"
            raise AssertionError(msg)

    @test(name="to_be_greater_than")
    def test_to_be_greater_than() -> None:
        expect(5).to_be_greater_than(3)
        expect(3).not_.to_be_greater_than(5)

    @test(name="to_be_less_than")
    def test_to_be_less_than() -> None:
        expect(3).to_be_less_than(5)
        expect(5).not_.to_be_less_than(3)

    @test(name="to_be_greater_than_or_equal")
    def test_to_be_greater_than_or_equal() -> None:
        expect(5).to_be_greater_than_or_equal(5)
        expect(6).to_be_greater_than_or_equal(5)
        expect(4).not_.to_be_greater_than_or_equal(5)

    @test(name="to_be_less_than_or_equal")
    def test_to_be_less_than_or_equal() -> None:
        expect(5).to_be_less_than_or_equal(5)
        expect(4).to_be_less_than_or_equal(5)
        expect(6).not_.to_be_less_than_or_equal(5)

    @test(name="to_contain")
    def test_to_contain() -> None:
        expect([1, 2, 3]).to_contain(2)
        expect("hello").to_contain("ell")
        expect([1, 2, 3]).not_.to_contain(4)

    @test(name="to_have_length")
    def test_to_have_length() -> None:
        expect([1, 2, 3]).to_have_length(3)
        expect("hello").to_have_length(5)
        expect([]).to_have_length(0)

    @test(name="to_match regex")
    def test_to_match() -> None:
        expect("hello world").to_match(r"hello")
        expect("foo123").to_match(r"\d+")
        expect("hello").not_.to_match(r"\d+")

    @test(name="not_ modifier negates matchers")
    def test_not_modifier() -> None:
        expect(1).not_.to_equal(2)
        expect("a").not_.to_be("b")
        expect(0).not_.to_be_truthy()
        expect(1).not_.to_be_falsy()

    @test(name="expectation error carries expected/received fields")
    def test_expectation_error_carries_fields() -> None:
        # isolate from the worker's soft context so the expected failure
        # doesn't pollute the test outcome.
        ctx = SoftContext()
        _set_soft_context(ctx)
        try:
            expect(True).to_be_falsy().fatal()  # noqa: FBT003
        except ExpectationError as exc:
            _set_soft_context(None)
            expect(exc.expected).to_equal("falsy")
            expect(exc.received).to_equal("True")
        else:
            _set_soft_context(None)
            msg = "ExpectationError was not raised"
            raise AssertionError(msg)

    @test(name="negated expectation error")
    def test_negated_expectation_error() -> None:
        ctx = SoftContext()
        _set_soft_context(ctx)
        try:
            expect(1).not_.to_equal(1).fatal()
        except ExpectationError as exc:
            _set_soft_context(None)
            expect(exc.expected).to_equal("not 1")
            expect(exc.received).to_equal("1")
        else:
            _set_soft_context(None)
            msg = "ExpectationError was not raised"
            raise AssertionError(msg)


with describe("soft assertions"):

    @test(name="soft assertions collect all failures")
    def test_soft_assertions_collect_all_failures() -> None:
        ctx = SoftContext()
        _set_soft_context(ctx)
        try:
            expect(1).to_equal(2)
            expect(3).to_equal(3)
            expect(4).to_equal(5)
        finally:
            _set_soft_context(None)
        expect(len(ctx.failures)).to_equal(2)
        expect(ctx.failures[0][0].expected).to_equal("2")
        expect(ctx.failures[1][0].expected).to_equal("5")

    @test(name="fatal() on passing assertion is a noop")
    def test_fatal_on_passing_assertion_is_noop() -> None:
        ctx = SoftContext()
        _set_soft_context(ctx)
        try:
            expect(1).to_equal(1).fatal()
        finally:
            _set_soft_context(None)
        expect(len(ctx.failures)).to_equal(0)

    @test(name="executed_lines tracks every expect() that ran")
    def test_executed_lines_tracks_all_runs() -> None:
        ctx = SoftContext()
        _set_soft_context(ctx)
        try:
            expect(1).to_equal(1)  # pass
            expect(2).to_equal(3)  # fail
            expect(4).to_equal(4)  # pass
        finally:
            _set_soft_context(None)
        expect(len(ctx.executed_lines)).to_equal(3)
        # Lines recorded in encounter order.
        expect(ctx.executed_lines[0]).to_be_less_than(ctx.executed_lines[1])
        expect(ctx.executed_lines[1]).to_be_less_than(ctx.executed_lines[2])

    @test(name="fatal() on failing assertion raises")
    def test_fatal_on_failing_assertion_raises() -> None:
        ctx = SoftContext()
        _set_soft_context(ctx)
        try:
            expect(1).to_equal(2).fatal()
        except ExpectationError as exc:
            _set_soft_context(None)
            expect(exc.expected).to_equal("2")
        else:
            _set_soft_context(None)
            msg = "ExpectationError was not raised by .fatal()"
            raise AssertionError(msg)

    @test(name="soft failures followed by fatal()")
    def test_soft_failures_then_fatal() -> None:
        ctx = SoftContext()
        _set_soft_context(ctx)
        try:
            expect(1).to_equal(99)
            expect(2).to_equal(98)
            expect(3).to_equal(97).fatal()
        except ExpectationError as exc:
            _set_soft_context(None)
            # .fatal() removes its own entry from ctx.failures before raising
            # so the test runner doesn't double-report it. The two prior soft
            # failures stay.
            expect(len(ctx.failures)).to_equal(2)
            expect(exc.expected).to_equal("97")
        else:
            _set_soft_context(None)
            msg = "ExpectationError was not raised by .fatal()"
            raise AssertionError(msg)

    @test(name="soft context captures caller frame")
    def test_soft_context_captures_caller_frame() -> None:
        ctx = SoftContext()
        _set_soft_context(ctx)
        try:
            expect(1).to_equal(2)
        finally:
            _set_soft_context(None)
        expect(len(ctx.failures)).to_equal(1)
        frame = ctx.failures[0][1]
        expect(frame).not_.to_be_none()
        if frame is None:
            msg = "frame should not be None"
            raise AssertionError(msg)
        expect(frame.filename).to_contain("test_expect.py")


with describe("to_raise"):

    @test(name="to_raise catches matching exception type")
    def test_to_raise_catches_matching_type() -> None:
        expect(lambda: (_ for _ in ()).throw(ValueError("boom"))).to_raise(ValueError)

    @test(name="to_raise catches any exception")
    def test_to_raise_catches_any_exception() -> None:
        def raises() -> None:
            msg = "oops"
            raise RuntimeError(msg)

        expect(raises).to_raise()

    @test(name="to_raise with match pattern")
    def test_to_raise_with_match_pattern() -> None:
        def raises() -> None:
            msg = "file not found: /tmp/foo"
            raise OSError(msg)

        expect(raises).to_raise(OSError, match=r"not found")

    @test(name="to_raise fails when no exception raised")
    def test_to_raise_fails_when_no_exception() -> None:
        ctx = SoftContext()
        _set_soft_context(ctx)
        try:
            expect(lambda: None).to_raise(ValueError).fatal()
        except ExpectationError as exc:
            _set_soft_context(None)
            expect(exc.received).to_equal("no exception")
        else:
            _set_soft_context(None)
            msg = "ExpectationError was not raised"
            raise AssertionError(msg)

    @test(name="to_raise fails when wrong exception type")
    def test_to_raise_fails_when_wrong_type() -> None:
        def raises() -> None:
            msg = "oops"
            raise TypeError(msg)

        ctx = SoftContext()
        _set_soft_context(ctx)
        try:
            expect(raises).to_raise(ValueError).fatal()
        except ExpectationError as exc:
            _set_soft_context(None)
            expect(exc.received).to_contain("TypeError")
        else:
            _set_soft_context(None)
            msg = "ExpectationError was not raised"
            raise AssertionError(msg)

    @test(name="not_.to_raise passes when no exception")
    def test_not_to_raise_passes_when_no_exception() -> None:
        expect(lambda: None).not_.to_raise()

    @test(name="not_.to_raise fails when exception raised")
    def test_not_to_raise_fails_when_exception() -> None:
        def raises() -> None:
            msg = "oops"
            raise RuntimeError(msg)

        ctx = SoftContext()
        _set_soft_context(ctx)
        try:
            expect(raises).not_.to_raise().fatal()
        except ExpectationError as exc:
            _set_soft_context(None)
            expect(exc.received).to_contain("RuntimeError")
        else:
            _set_soft_context(None)
            msg = "ExpectationError was not raised"
            raise AssertionError(msg)

    @test(name="to_raise raises TypeError for non-callable")
    def test_to_raise_raises_type_error_for_non_callable() -> None:
        # Access to_raise via getattr to bypass the static protocol bound —
        # this tests the runtime TypeError guard for non-callable values.
        to_raise = getattr(Expectation(42), "to_raise")  # noqa: B009
        expect(lambda: to_raise(ValueError)).to_raise(TypeError, match="callable")


with describe("markers"):

    @test(name="skip marker stamps __tryke_skip__")
    def test_skip_marker_stamps_dunder() -> None:
        @test.skip
        def skipped() -> None:
            pass

        expect(hasattr(skipped, "__tryke_skip__")).to_be_truthy()
        expect(skipped.__tryke_skip__).to_equal("")

    @test(name="skip marker with reason")
    def test_skip_marker_with_reason() -> None:
        @test.skip("broken")
        def skipped() -> None:
            pass

        expect(skipped.__tryke_skip__).to_equal("broken")

    @test(name="todo marker stamps __tryke_todo__")
    def test_todo_marker_stamps_dunder() -> None:
        @test.todo
        def pending() -> None:
            pass

        expect(hasattr(pending, "__tryke_todo__")).to_be_truthy()
        expect(pending.__tryke_todo__).to_equal("")

    @test(name="todo marker with description")
    def test_todo_marker_with_description() -> None:
        @test.todo("need caching")
        def pending() -> None:
            pass

        expect(pending.__tryke_todo__).to_equal("need caching")

    @test(name="xfail marker stamps __tryke_xfail__")
    def test_xfail_marker_stamps_dunder() -> None:
        @test.xfail
        def expected_fail() -> None:
            pass

        expect(hasattr(expected_fail, "__tryke_xfail__")).to_be_truthy()
        expect(expected_fail.__tryke_xfail__).to_equal("")

    @test(name="xfail marker with reason")
    def test_xfail_marker_with_reason() -> None:
        @test.xfail("upstream bug")
        def expected_fail() -> None:
            pass

        expect(expected_fail.__tryke_xfail__).to_equal("upstream bug")

    @test(name="skip marker accepts name kwarg")
    def test_skip_marker_with_name_kwarg() -> None:
        @test.skip(name="my skip label")
        def skipped() -> None:
            pass

        expect(hasattr(skipped, "__tryke_skip__")).to_be_truthy()

    @test(name="todo marker accepts name kwarg")
    def test_todo_marker_with_name_kwarg() -> None:
        @test.todo(name="my todo label")
        def pending() -> None:
            pass

        expect(hasattr(pending, "__tryke_todo__")).to_be_truthy()

    @test(name="xfail marker accepts name kwarg")
    def test_xfail_marker_with_name_kwarg() -> None:
        @test.xfail(name="my xfail label")
        def expected_fail() -> None:
            pass

        expect(hasattr(expected_fail, "__tryke_xfail__")).to_be_truthy()

    @test(name="skip_if(true) stamps __tryke_skip__")
    def test_skip_if_true_stamps_dunder() -> None:
        @test.skip_if(True, reason="always skip")  # noqa: FBT003
        def skipped() -> None:
            pass

        expect(hasattr(skipped, "__tryke_skip__")).to_be_truthy()
        if not isinstance(skipped, _SkipMarked):
            msg = "skip_if should stamp __tryke_skip__"
            raise TypeError(msg)
        expect(skipped.__tryke_skip__).to_equal("always skip")

    @test(name="skip_if(false) does not stamp")
    def test_skip_if_false_does_not_stamp() -> None:
        @test.skip_if(False, reason="never skip")  # noqa: FBT003
        def not_skipped() -> None:
            pass

        expect(hasattr(not_skipped, "__tryke_skip__")).to_be_falsy()


with describe("async"):

    @test(name="async test basic")
    async def test_async_basic() -> None:
        expect(1 + 1).to_equal(2)

    @test(name="async test with await")
    async def test_async_with_await() -> None:
        await asyncio.sleep(0)
        expect(True).to_be_truthy()  # noqa: FBT003


with describe("doctests"):

    @test(name="MatchResult repr shows ok for passing assertion")
    def test_match_result_repr_ok() -> None:
        result = expect(1).to_equal(1)
        expect(repr(result)).to_equal("MatchResult(ok)")

    @test(name="MatchResult repr shows failed for failing assertion")
    def test_match_result_repr_failed() -> None:
        # MatchResult(failed) is only observable in soft-assertion context;
        # outside soft context, a failing assertion raises immediately.
        result = MatchResult(None)
        expect(repr(result)).to_equal("MatchResult(ok)")
        err = ExpectationError("x", expected="1", received="2")
        result_failed = MatchResult(err)
        expect(repr(result_failed)).to_equal("MatchResult(failed)")

    @test(name="MatchResult __repr__ is defined")
    def test_match_result_has_repr() -> None:
        expect(hasattr(MatchResult, "__repr__")).to_be_truthy()

    def add(a: int, b: int) -> int:
        """Add two numbers.

        >>> add(1, 2)
        3
        >>> add(0, 0)
        0
        """
        return a + b

    def greet(name: str) -> str:
        """Greet someone.

        >>> greet("world")
        'hello, world'
        """
        return f"hello, {name}"

    class Counter:
        """A simple counter.

        >>> c = Counter()
        >>> c.value
        0
        """

        def __init__(self) -> None:
            self.value = 0

        def increment(self) -> None:
            """Increment the counter.

            >>> c = Counter()
            >>> c.increment()
            >>> c.value
            1
            """
            self.value += 1


with describe("benchmark summary"):

    def _load_module() -> ModuleType:
        path = Path(__file__).resolve().parent.parent / "benchmarks" / "summarize.py"
        spec = importlib.util.spec_from_file_location("benchmark_summarize", path)
        if spec is None or spec.loader is None:
            msg = "failed to load benchmark summarizer"
            raise RuntimeError(msg)

        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module

    def _write_json(path: Path, payload: dict) -> None:
        path.write_text(json.dumps(payload), encoding="utf-8")

    def _benchmark_payload(tryke_mean: float, pytest_mean: float) -> dict:
        return {
            "results": [
                {"mean": tryke_mean},
                {"mean": pytest_mean},
            ]
        }

    @test(name="benchmark summarize embeds generated docs block")
    def test_generate_outputs_updates_results_and_docs() -> None:
        summarize = _load_module()

        with TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            results_dir = root / "results"
            results_dir.mkdir()

            for stem, values in {
                "discovery_50": (0.1748, 0.1996),
                "sequential_50": (0.2314, 0.2397),
                "parallel_50": (0.2901, 1.02),
            }.items():
                _write_json(results_dir / f"{stem}.json", _benchmark_payload(*values))

            _write_json(
                results_dir / "system.json",
                {
                    "platform": {
                        "system": "Linux",
                        "release": "Ubuntu 24.04",
                        "architecture": "x86_64",
                    },
                    "cpu": {
                        "model": "Example CPU",
                        "logical_cores": 8,
                    },
                    "versions": {
                        "python": "3.13.2",
                        "hyperfine": "hyperfine 1.19.0",
                        "tryke": "tryke 0.1.0",
                        "pytest": "9.0.2",
                        "pytest_xdist": "3.8.0",
                    },
                    "benchmark": {
                        "generated_at": "2026-03-12T12:00:00+00:00",
                        "warmup": 2,
                        "min_runs": 5,
                    },
                },
            )

            docs_path = root / "benchmarks.md"
            docs_path.write_text(
                f"# Benchmarks\n\n{summarize.DOCS_START_MARKER}\n_old_\n"
                f"{summarize.DOCS_END_MARKER}\n",
                encoding="utf-8",
            )

            outputs = summarize.generate_outputs(
                results_dir=results_dir, docs_path=docs_path
            )

            results_markdown = outputs[summarize.RESULTS_OUTPUT]
            docs_markdown = outputs[docs_path]

            expect(results_markdown).to_contain("# Benchmark Results")
            expect(results_markdown).to_contain("## Benchmark Environment")
            expect(results_markdown).to_contain("Example CPU (8 logical cores)")
            expect(results_markdown).to_contain("| 50 | 174.8ms | 199.6ms | 1.1x |")
            expect(docs_markdown).to_contain(summarize.DOCS_START_MARKER)
            expect(docs_markdown).to_contain("tryke 0.1.0")
            expect(docs_markdown).to_contain(summarize.DOCS_END_MARKER)

    @test(name="benchmark summarize tolerates missing system metadata")
    def test_render_results_sections_without_metadata() -> None:
        summarize = _load_module()

        with TemporaryDirectory() as tmpdir:
            results_dir = Path(tmpdir)
            _write_json(
                results_dir / "discovery_50.json", _benchmark_payload(0.05, 0.10)
            )

            rendered = summarize.render_results_sections(results_dir)

            expect(rendered).to_contain("System metadata unavailable")
            expect(rendered).to_contain("| 50 | 50.0ms | 100.0ms | 2.0x |")

    @test(name="benchmark summarize requires doc markers")
    def test_update_docs_markdown_requires_markers() -> None:
        summarize = _load_module()

        try:
            summarize.update_docs_markdown("# Benchmarks\n", "generated")
        except ValueError as exc:
            expect(str(exc)).to_contain("markers")
        else:
            msg = "expected missing-marker error"
            raise AssertionError(msg)
