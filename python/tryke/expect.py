from __future__ import annotations

import re
import traceback
from pathlib import Path
from typing import TYPE_CHECKING, overload

if TYPE_CHECKING:
    from collections.abc import Callable


@overload
def test(fn: Callable[[], None], /) -> Callable[[], None]: ...
@overload
def test(name: str, /) -> Callable[[Callable[[], None]], Callable[[], None]]: ...
@overload
def test(*, name: str) -> Callable[[Callable[[], None]], Callable[[], None]]: ...


def test(fn=None, /, *, name=None):  # noqa: PT028, ARG001
    if callable(fn):
        return fn

    def decorator(f: Callable[[], None]) -> Callable[[], None]:
        return f

    return decorator


class ExpectationError(AssertionError):
    def __init__(self, message: str, *, expected: str, received: str) -> None:
        super().__init__(message)
        self.expected = expected
        self.received = received


_TRYKE_PKG = str(Path(__file__).resolve().parent)


class MatchResult:
    def __init__(self, error: ExpectationError | None) -> None:
        self._error = error

    def fatal(self) -> None:
        """If this assertion failed, raise immediately (stop the test)."""
        if self._error is not None:
            raise self._error


class SoftContext:
    def __init__(self) -> None:
        self.failures: list[tuple[ExpectationError, traceback.FrameSummary | None]] = []


_soft_context: SoftContext | None = None


def _caller_frame() -> traceback.FrameSummary | None:
    for frame in reversed(traceback.extract_stack()):
        if not str(Path(frame.filename).resolve()).startswith(_TRYKE_PKG):
            return frame
    return None


class Expectation[T]:
    def __init__(self, value: T, *, negated: bool = False) -> None:
        self._value: T = value
        self._negated: bool = negated

    @property
    def not_(self) -> Expectation[T]:
        return Expectation(self._value, negated=not self._negated)

    def _assert(
        self,
        passed: bool,  # noqa: FBT001
        message: str,
        *,
        expected: str,
        received: str,
    ) -> MatchResult:
        ok = (not passed) if self._negated else passed
        if not ok:
            prefix = "expected not " if self._negated else "expected "
            actual_expected = ("not " + expected) if self._negated else expected
            err = ExpectationError(
                prefix + message, expected=actual_expected, received=received
            )
            if _soft_context is not None:
                frame = _caller_frame()
                _soft_context.failures.append((err, frame))
                return MatchResult(err)
            raise err
        return MatchResult(None)

    def to_equal(self, other: T) -> MatchResult:
        return self._assert(
            self._value == other,
            f"{self._value!r} to equal {other!r}",
            expected=repr(other),
            received=repr(self._value),
        )

    def to_be(self, other: object) -> MatchResult:
        return self._assert(
            self._value is other,
            f"{self._value!r} to be {other!r}",
            expected=repr(other),
            received=repr(self._value),
        )

    def to_be_truthy(self) -> MatchResult:
        return self._assert(
            bool(self._value),
            f"{self._value!r} to be truthy",
            expected="truthy",
            received=repr(self._value),
        )

    def to_be_falsy(self) -> MatchResult:
        return self._assert(
            not bool(self._value),
            f"{self._value!r} to be falsy",
            expected="falsy",
            received=repr(self._value),
        )

    def to_be_none(self) -> MatchResult:
        return self._assert(
            self._value is None,
            f"{self._value!r} to be None",
            expected="None",
            received=repr(self._value),
        )

    def to_be_greater_than(self, n: T) -> MatchResult:
        return self._assert(
            self._value > n,
            f"{self._value!r} to be greater than {n!r}",
            expected=f"> {n!r}",
            received=repr(self._value),
        )  # type: ignore[operator]

    def to_be_less_than(self, n: T) -> MatchResult:
        return self._assert(
            self._value < n,
            f"{self._value!r} to be less than {n!r}",
            expected=f"< {n!r}",
            received=repr(self._value),
        )  # type: ignore[operator]

    def to_be_greater_than_or_equal(self, n: T) -> MatchResult:
        return self._assert(
            self._value >= n,  # type: ignore[operator]
            f"{self._value!r} to be greater than or equal to {n!r}",
            expected=f">= {n!r}",
            received=repr(self._value),
        )

    def to_be_less_than_or_equal(self, n: T) -> MatchResult:
        return self._assert(
            self._value <= n,  # type: ignore[operator]
            f"{self._value!r} to be less than or equal to {n!r}",
            expected=f"<= {n!r}",
            received=repr(self._value),
        )

    def to_contain(self, item: T) -> MatchResult:
        return self._assert(
            item in self._value,
            f"{self._value!r} to contain {item!r}",
            expected=f"contains {item!r}",
            received=repr(self._value),
        )  # type: ignore[operator]

    def to_have_length(self, n: int) -> MatchResult:
        actual = len(self._value)  # type: ignore[arg-type]
        return self._assert(
            actual == n,
            f"{self._value!r} to have length {n}, got {actual}",
            expected=f"length {n}",
            received=f"length {actual}",
        )

    def to_match(self, pattern: str) -> MatchResult:
        return self._assert(
            bool(re.search(pattern, str(self._value))),
            f"{self._value!r} to match pattern {pattern!r}",
            expected=f"matches {pattern!r}",
            received=repr(self._value),
        )


# `name` is unused at runtime — it exists as an AST-level metadata carrier
# so the Rust-side discovery can extract assertion labels from source code.
def expect[T](expr: T, name: str | None = None) -> Expectation[T]:  # noqa: ARG001
    return Expectation(expr)


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
    _true = True
    try:
        expect(_true).to_be_falsy()
    except ExpectationError as exc:
        expect(exc.expected).to_equal("falsy")
        expect(exc.received).to_equal("True")
    else:
        msg = "ExpectationError was not raised"
        raise AssertionError(msg)


@test
def test_negated_expectation_error() -> None:
    try:
        expect(1).not_.to_equal(1)
    except ExpectationError as exc:
        expect(exc.expected).to_equal("not 1")
        expect(exc.received).to_equal("1")
    else:
        msg = "ExpectationError was not raised"
        raise AssertionError(msg)


@test
def test_soft_assertions_collect_all_failures() -> None:
    global _soft_context  # noqa: PLW0603
    ctx = SoftContext()
    _soft_context = ctx
    try:
        expect(1).to_equal(2)
        expect(3).to_equal(3)
        expect(4).to_equal(5)
    finally:
        _soft_context = None
    expect(len(ctx.failures)).to_equal(2)
    expect(ctx.failures[0][0].expected).to_equal("2")
    expect(ctx.failures[1][0].expected).to_equal("5")


@test
def test_fatal_on_passing_assertion_is_noop() -> None:
    global _soft_context  # noqa: PLW0603
    ctx = SoftContext()
    _soft_context = ctx
    try:
        expect(1).to_equal(1).fatal()
    finally:
        _soft_context = None
    expect(len(ctx.failures)).to_equal(0)


@test
def test_fatal_on_failing_assertion_raises() -> None:
    global _soft_context  # noqa: PLW0603
    ctx = SoftContext()
    _soft_context = ctx
    try:
        expect(1).to_equal(2).fatal()
    except ExpectationError as exc:
        _soft_context = None
        expect(exc.expected).to_equal("2")
    else:
        _soft_context = None
        msg = "ExpectationError was not raised by .fatal()"
        raise AssertionError(msg)


@test
def test_soft_failures_then_fatal() -> None:
    global _soft_context  # noqa: PLW0603
    ctx = SoftContext()
    _soft_context = ctx
    try:
        expect(1).to_equal(99)
        expect(2).to_equal(98)
        expect(3).to_equal(97).fatal()
    except ExpectationError as exc:
        _soft_context = None
        expect(len(ctx.failures)).to_equal(3)
        expect(exc.expected).to_equal("97")
    else:
        _soft_context = None
        msg = "ExpectationError was not raised by .fatal()"
        raise AssertionError(msg)


@test
def test_soft_context_captures_caller_frame() -> None:
    global _soft_context  # noqa: PLW0603
    ctx = SoftContext()
    _soft_context = ctx
    try:
        expect(1).to_equal(2)
    finally:
        _soft_context = None
    expect(len(ctx.failures)).to_equal(1)
    frame = ctx.failures[0][1]
    expect(frame).not_.to_be_none()
    expect(frame.filename).to_contain("expect.py")
