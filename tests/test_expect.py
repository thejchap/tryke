from __future__ import annotations

import asyncio
import sys

from tryke import expect, test
from tryke.expect import ExpectationError, SoftContext

_expect_mod = sys.modules["tryke.expect"]


@test
def test_basic() -> None:
    expect(1).to_equal(1)
    expect("hello").to_equal("hello")


@test
def test_to_be() -> None:
    sentinel = object()
    expect(sentinel).to_be(sentinel)
    expect(None).to_be(None)


@test
def test_to_be_truthy() -> None:
    expect(1).to_be_truthy()
    expect("x").to_be_truthy()
    expect([1]).to_be_truthy()


@test
def test_to_be_falsy() -> None:
    expect(0).to_be_falsy()
    expect("").to_be_falsy()
    expect([]).to_be_falsy()


@test
def test_to_be_none() -> None:
    expect(None).to_be_none()
    expect(1).not_.to_be_none()


@test
def test_to_be_greater_than() -> None:
    expect(5).to_be_greater_than(3)
    expect(3).not_.to_be_greater_than(5)


@test
def test_to_be_less_than() -> None:
    expect(3).to_be_less_than(5)
    expect(5).not_.to_be_less_than(3)


@test
def test_to_be_greater_than_or_equal() -> None:
    expect(5).to_be_greater_than_or_equal(5)
    expect(6).to_be_greater_than_or_equal(5)
    expect(4).not_.to_be_greater_than_or_equal(5)


@test
def test_to_be_less_than_or_equal() -> None:
    expect(5).to_be_less_than_or_equal(5)
    expect(4).to_be_less_than_or_equal(5)
    expect(6).not_.to_be_less_than_or_equal(5)


@test
def test_to_contain() -> None:
    expect([1, 2, 3]).to_contain(2)
    expect("hello").to_contain("ell")
    expect([1, 2, 3]).not_.to_contain(4)


@test
def test_to_have_length() -> None:
    expect([1, 2, 3]).to_have_length(3)
    expect("hello").to_have_length(5)
    expect([]).to_have_length(0)


@test
def test_to_match() -> None:
    expect("hello world").to_match(r"hello")
    expect("foo123").to_match(r"\d+")
    expect("hello").not_.to_match(r"\d+")


@test
def test_not_modifier() -> None:
    expect(1).not_.to_equal(2)
    expect("a").not_.to_be("b")
    expect(0).not_.to_be_truthy()
    expect(1).not_.to_be_falsy()


@test
def test_expectation_error_carries_fields() -> None:
    # Isolate from the worker's soft context so the expected failure
    # doesn't pollute the test outcome.
    ctx = SoftContext()
    _expect_mod._soft_context = ctx  # noqa: SLF001
    try:
        expect(True).to_be_falsy().fatal()  # noqa: FBT003
    except ExpectationError as exc:
        _expect_mod._soft_context = None  # noqa: SLF001
        expect(exc.expected).to_equal("falsy")
        expect(exc.received).to_equal("True")
    else:
        _expect_mod._soft_context = None  # noqa: SLF001
        msg = "ExpectationError was not raised"
        raise AssertionError(msg)


@test
def test_negated_expectation_error() -> None:
    ctx = SoftContext()
    _expect_mod._soft_context = ctx  # noqa: SLF001
    try:
        expect(1).not_.to_equal(1).fatal()
    except ExpectationError as exc:
        _expect_mod._soft_context = None  # noqa: SLF001
        expect(exc.expected).to_equal("not 1")
        expect(exc.received).to_equal("1")
    else:
        _expect_mod._soft_context = None  # noqa: SLF001
        msg = "ExpectationError was not raised"
        raise AssertionError(msg)


@test
def test_soft_assertions_collect_all_failures() -> None:
    ctx = SoftContext()
    _expect_mod._soft_context = ctx  # noqa: SLF001
    try:
        expect(1).to_equal(2)
        expect(3).to_equal(3)
        expect(4).to_equal(5)
    finally:
        _expect_mod._soft_context = None  # noqa: SLF001
    expect(len(ctx.failures)).to_equal(2)
    expect(ctx.failures[0][0].expected).to_equal("2")
    expect(ctx.failures[1][0].expected).to_equal("5")


@test
def test_fatal_on_passing_assertion_is_noop() -> None:
    ctx = SoftContext()
    _expect_mod._soft_context = ctx  # noqa: SLF001
    try:
        expect(1).to_equal(1).fatal()
    finally:
        _expect_mod._soft_context = None  # noqa: SLF001
    expect(len(ctx.failures)).to_equal(0)


@test
def test_fatal_on_failing_assertion_raises() -> None:
    ctx = SoftContext()
    _expect_mod._soft_context = ctx  # noqa: SLF001
    try:
        expect(1).to_equal(2).fatal()
    except ExpectationError as exc:
        _expect_mod._soft_context = None  # noqa: SLF001
        expect(exc.expected).to_equal("2")
    else:
        _expect_mod._soft_context = None  # noqa: SLF001
        msg = "ExpectationError was not raised by .fatal()"
        raise AssertionError(msg)


@test
def test_soft_failures_then_fatal() -> None:
    ctx = SoftContext()
    _expect_mod._soft_context = ctx  # noqa: SLF001
    try:
        expect(1).to_equal(99)
        expect(2).to_equal(98)
        expect(3).to_equal(97).fatal()
    except ExpectationError as exc:
        _expect_mod._soft_context = None  # noqa: SLF001
        expect(len(ctx.failures)).to_equal(3)
        expect(exc.expected).to_equal("97")
    else:
        _expect_mod._soft_context = None  # noqa: SLF001
        msg = "ExpectationError was not raised by .fatal()"
        raise AssertionError(msg)


@test
def test_soft_context_captures_caller_frame() -> None:
    ctx = SoftContext()
    _expect_mod._soft_context = ctx  # noqa: SLF001
    try:
        expect(1).to_equal(2)
    finally:
        _expect_mod._soft_context = None  # noqa: SLF001
    expect(len(ctx.failures)).to_equal(1)
    frame = ctx.failures[0][1]
    expect(frame).not_.to_be_none()
    expect(frame.filename).to_contain("test_expect.py")


# --- to_raise tests ---


@test
def test_to_raise_catches_matching_type() -> None:
    expect(lambda: (_ for _ in ()).throw(ValueError("boom"))).to_raise(ValueError)


@test
def test_to_raise_catches_any_exception() -> None:
    def raises() -> None:
        msg = "oops"
        raise RuntimeError(msg)

    expect(raises).to_raise()


@test
def test_to_raise_with_match_pattern() -> None:
    def raises() -> None:
        msg = "file not found: /tmp/foo"
        raise OSError(msg)

    expect(raises).to_raise(OSError, match=r"not found")


@test
def test_to_raise_fails_when_no_exception() -> None:
    ctx = SoftContext()
    _expect_mod._soft_context = ctx  # noqa: SLF001
    try:
        expect(lambda: None).to_raise(ValueError).fatal()
    except ExpectationError as exc:
        _expect_mod._soft_context = None  # noqa: SLF001
        expect(exc.received).to_equal("no exception")
    else:
        _expect_mod._soft_context = None  # noqa: SLF001
        msg = "ExpectationError was not raised"
        raise AssertionError(msg)


@test
def test_to_raise_fails_when_wrong_type() -> None:
    def raises() -> None:
        msg = "oops"
        raise TypeError(msg)

    ctx = SoftContext()
    _expect_mod._soft_context = ctx  # noqa: SLF001
    try:
        expect(raises).to_raise(ValueError).fatal()
    except ExpectationError as exc:
        _expect_mod._soft_context = None  # noqa: SLF001
        expect(exc.received).to_contain("TypeError")
    else:
        _expect_mod._soft_context = None  # noqa: SLF001
        msg = "ExpectationError was not raised"
        raise AssertionError(msg)


@test
def test_not_to_raise_passes_when_no_exception() -> None:
    expect(lambda: None).not_.to_raise()


@test
def test_not_to_raise_fails_when_exception() -> None:
    def raises() -> None:
        msg = "oops"
        raise RuntimeError(msg)

    ctx = SoftContext()
    _expect_mod._soft_context = ctx  # noqa: SLF001
    try:
        expect(raises).not_.to_raise().fatal()
    except ExpectationError as exc:
        _expect_mod._soft_context = None  # noqa: SLF001
        expect(exc.received).to_contain("RuntimeError")
    else:
        _expect_mod._soft_context = None  # noqa: SLF001
        msg = "ExpectationError was not raised"
        raise AssertionError(msg)


@test
def test_to_raise_raises_type_error_for_non_callable() -> None:
    try:
        expect(42).to_raise(ValueError)
    except TypeError as exc:
        expect(str(exc)).to_contain("callable")
    else:
        msg = "TypeError was not raised"
        raise AssertionError(msg)


# --- _TestBuilder marker tests ---


@test
def test_skip_marker_stamps_dunder() -> None:
    @test.skip
    def skipped() -> None:
        pass

    expect(hasattr(skipped, "__tryke_skip__")).to_be_truthy()
    expect(skipped.__tryke_skip__).to_equal("")


@test
def test_skip_marker_with_reason() -> None:
    @test.skip("broken")
    def skipped() -> None:
        pass

    expect(skipped.__tryke_skip__).to_equal("broken")


@test
def test_todo_marker_stamps_dunder() -> None:
    @test.todo
    def pending() -> None:
        pass

    expect(hasattr(pending, "__tryke_todo__")).to_be_truthy()
    expect(pending.__tryke_todo__).to_equal("")


@test
def test_todo_marker_with_description() -> None:
    @test.todo("need caching")
    def pending() -> None:
        pass

    expect(pending.__tryke_todo__).to_equal("need caching")


@test
def test_xfail_marker_stamps_dunder() -> None:
    @test.xfail
    def expected_fail() -> None:
        pass

    expect(hasattr(expected_fail, "__tryke_xfail__")).to_be_truthy()
    expect(expected_fail.__tryke_xfail__).to_equal("")


@test
def test_xfail_marker_with_reason() -> None:
    @test.xfail("upstream bug")
    def expected_fail() -> None:
        pass

    expect(expected_fail.__tryke_xfail__).to_equal("upstream bug")


@test
def test_skip_if_true_stamps_dunder() -> None:
    @test.skip_if(True, reason="always skip")  # noqa: FBT003
    def skipped() -> None:
        pass

    expect(hasattr(skipped, "__tryke_skip__")).to_be_truthy()
    expect(skipped.__tryke_skip__).to_equal("always skip")


@test
def test_skip_if_false_does_not_stamp() -> None:
    @test.skip_if(False, reason="never skip")  # noqa: FBT003
    def not_skipped() -> None:
        pass

    expect(hasattr(not_skipped, "__tryke_skip__")).to_be_falsy()


# --- async test support ---


@test
async def test_async_basic() -> None:
    expect(1 + 1).to_equal(2)


@test
async def test_async_with_await() -> None:
    await asyncio.sleep(0)
    expect(True).to_be_truthy()  # noqa: FBT003
