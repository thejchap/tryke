from __future__ import annotations

import re
import traceback
from pathlib import Path
from typing import TYPE_CHECKING, overload

if TYPE_CHECKING:
    from collections.abc import Callable

type _Fn = Callable[[], None]
type _Decorator = Callable[[_Fn], _Fn]


class _SkipMarker:
    def __get__(self, obj: object, objtype: type | None = None) -> _SkipMarker:
        return self

    def __call__(
        self,
        fn_or_reason: _Fn | str | None = None,
        /,
        *,
        reason: str | None = None,
        tags: list[str] | None = None,  # noqa: ARG002
    ) -> _Fn | _Decorator:
        if callable(fn_or_reason):
            fn_or_reason.__tryke_skip__ = ""  # type: ignore[attr-defined]
            return fn_or_reason
        actual_reason = fn_or_reason or reason or ""

        def decorator(f: _Fn) -> _Fn:
            f.__tryke_skip__ = actual_reason  # type: ignore[attr-defined]
            return f

        return decorator


class _TodoMarker:
    def __get__(self, obj: object, objtype: type | None = None) -> _TodoMarker:
        return self

    def __call__(
        self,
        fn_or_desc: _Fn | str | None = None,
        /,
        *,
        description: str | None = None,
        tags: list[str] | None = None,  # noqa: ARG002
    ) -> _Fn | _Decorator:
        if callable(fn_or_desc):
            fn_or_desc.__tryke_todo__ = ""  # type: ignore[attr-defined]
            return fn_or_desc
        actual_desc = fn_or_desc or description or ""

        def decorator(f: _Fn) -> _Fn:
            f.__tryke_todo__ = actual_desc  # type: ignore[attr-defined]
            return f

        return decorator


class _XfailMarker:
    def __get__(self, obj: object, objtype: type | None = None) -> _XfailMarker:
        return self

    def __call__(
        self,
        fn_or_reason: _Fn | str | None = None,
        /,
        *,
        reason: str | None = None,
        tags: list[str] | None = None,  # noqa: ARG002
    ) -> _Fn | _Decorator:
        if callable(fn_or_reason):
            fn_or_reason.__tryke_xfail__ = ""  # type: ignore[attr-defined]
            return fn_or_reason
        actual_reason = fn_or_reason or reason or ""

        def decorator(f: _Fn) -> _Fn:
            f.__tryke_xfail__ = actual_reason  # type: ignore[attr-defined]
            return f

        return decorator


class _TestBuilder:
    skip = _SkipMarker()
    todo = _TodoMarker()
    xfail = _XfailMarker()

    @overload
    def __call__(self, fn: Callable[[], None], /) -> Callable[[], None]: ...
    @overload
    def __call__(
        self, name: str, /
    ) -> Callable[[Callable[[], None]], Callable[[], None]]: ...
    @overload
    def __call__(
        self, *, name: str
    ) -> Callable[[Callable[[], None]], Callable[[], None]]: ...

    def __call__(self, fn=None, /, *, name=None, tags=None):  # noqa: ARG002
        if callable(fn):
            return fn

        def decorator(f: Callable[[], None]) -> Callable[[], None]:
            return f

        return decorator

    def skip_if(self, condition: bool, *, reason: str = "") -> _Decorator:  # noqa: FBT001
        """Conditional skip, evaluated at import time."""

        def decorator(f: _Fn) -> _Fn:
            if condition:
                f.__tryke_skip__ = reason  # type: ignore[attr-defined]
            return f

        return decorator


test = _TestBuilder()


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

    def to_raise(
        self,
        exc_type: type[BaseException] | None = None,
        *,
        match: str | None = None,
    ) -> MatchResult:
        if not callable(self._value):
            msg = "to_raise() requires a callable; wrap the expression in a lambda"
            raise TypeError(msg)

        try:
            self._value()
        except BaseException as exc:  # noqa: BLE001
            caught = exc
        else:
            caught = None

        raised = caught is not None
        type_ok = exc_type is None or (
            caught is not None and isinstance(caught, exc_type)
        )
        match_ok = match is None or (
            caught is not None and bool(re.search(match, str(caught)))
        )
        passed = raised and type_ok and match_ok

        if exc_type is not None:
            expected_str = exc_type.__name__
            if match:
                expected_str += f" matching {match!r}"
        else:
            expected_str = "any exception"

        if caught is not None:
            received_str = f"{type(caught).__name__}: {caught}"
        else:
            received_str = "no exception"

        return self._assert(
            passed,
            f"callable to raise {expected_str}",
            expected=expected_str,
            received=received_str,
        )


# `name` is unused at runtime — it exists as an AST-level metadata carrier
# so the Rust-side discovery can extract assertion labels from source code.
def expect[T](expr: T, name: str | None = None) -> Expectation[T]:  # noqa: ARG001
    return Expectation(expr)
