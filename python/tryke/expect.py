"""Core test and assertion API for tryke."""

from __future__ import annotations

import re
import traceback
from pathlib import Path
from typing import TYPE_CHECKING, NamedTuple, Protocol, overload, runtime_checkable

if TYPE_CHECKING:
    from collections.abc import Callable, Coroutine
    from typing import Any, ClassVar, Self

    class _SupportsGT(Protocol):
        def __gt__(self, other: Self, /) -> bool: ...

    class _SupportsLT(Protocol):
        def __lt__(self, other: Self, /) -> bool: ...

    class _SupportsGE(Protocol):
        def __ge__(self, other: Self, /) -> bool: ...

    class _SupportsLE(Protocol):
        def __le__(self, other: Self, /) -> bool: ...

    class _SupportsContains(Protocol):
        # str.__contains__ accepts str (not object), so the param must be
        # contravariant-friendly — Any is the only correct bound here.
        def __contains__(self, x: Any, /) -> bool: ...  # noqa: ANN401

    class _SupportsLen(Protocol):
        def __len__(self) -> int: ...

    class _SupportsCall(Protocol):
        def __call__(self) -> object: ...


type _Fn = Callable[[], None]
type _AsyncFn = Callable[[], Coroutine[Any, Any, None]]
type _AnyTestFn = _Fn | _AsyncFn
type _Decorator = Callable[[_AnyTestFn], _AnyTestFn]


@runtime_checkable
class _SkipMarked(Protocol):
    """A function stamped with ``__tryke_skip__``."""

    def __call__(self) -> None: ...

    __tryke_skip__: str


@runtime_checkable
class _TodoMarked(Protocol):
    """A function stamped with ``__tryke_todo__``."""

    def __call__(self) -> None: ...

    __tryke_todo__: str


@runtime_checkable
class _XfailMarked(Protocol):
    """A function stamped with ``__tryke_xfail__``."""

    def __call__(self) -> None: ...

    __tryke_xfail__: str


def _stamp(fn: object, attr: str, value: str) -> None:
    """Stamp a marker attribute on a function object."""
    setattr(fn, attr, value)


class _Marker:
    """Base for test markers that stamp a dunder attribute on the decorated function."""

    _attr: ClassVar[str]

    def __init_subclass__(cls, *, attr: str, **kwargs: object) -> None:
        super().__init_subclass__(**kwargs)
        cls._attr = attr

    def __get__(self, obj: object, objtype: type | None = None) -> Self:
        return self


class _SkipMarker(_Marker, attr="__tryke_skip__"):
    """Skip a test unconditionally.

    Can be used as a bare decorator or called with a reason string.

    Example:
        ```python
        @test.skip
        def not_ready():
            ...

        @test.skip("waiting on upstream fix")
        def with_reason():
            ...
        ```
    """

    @overload
    def __call__(self, fn_or_reason: _Fn, /) -> _SkipMarked: ...
    @overload
    def __call__(
        self,
        fn_or_reason: str | None = ...,
        /,
        *,
        reason: str | None = ...,
        name: str | None = ...,
        tags: list[str] | None = ...,
    ) -> Callable[[_Fn], _SkipMarked]: ...

    def __call__(
        self,
        fn_or_reason: _Fn | str | None = None,
        /,
        *,
        reason: str | None = None,
        name: str | None = None,  # noqa: ARG002
        tags: list[str] | None = None,  # noqa: ARG002
    ) -> object:
        """Mark a test to be skipped.

        Args:
            fn_or_reason: The test function (when used as a bare decorator)
                or a reason string (when called with parentheses).
            reason: Reason for skipping (alternative to positional string).
            name: Optional test name override.
            tags: Optional list of tags for filtering.
        """
        if callable(fn_or_reason):
            _stamp(fn_or_reason, self._attr, "")
            return fn_or_reason
        resolved = fn_or_reason or reason or ""

        def decorator(f: _Fn) -> _Fn:
            _stamp(f, self._attr, resolved)
            return f

        return decorator


class _TodoMarker(_Marker, attr="__tryke_todo__"):
    """Mark a test as a placeholder — it will be collected but not executed.

    Can be used as a bare decorator or called with a description string.

    Example:
        ```python
        @test.todo
        def future_feature():
            ...

        @test.todo("implement caching layer")
        def with_description():
            ...
        ```
    """

    @overload
    def __call__(self, fn_or_desc: _Fn, /) -> _TodoMarked: ...
    @overload
    def __call__(
        self,
        fn_or_desc: str | None = ...,
        /,
        *,
        description: str | None = ...,
        name: str | None = ...,
        tags: list[str] | None = ...,
    ) -> Callable[[_Fn], _TodoMarked]: ...

    def __call__(
        self,
        fn_or_desc: _Fn | str | None = None,
        /,
        *,
        description: str | None = None,
        name: str | None = None,  # noqa: ARG002
        tags: list[str] | None = None,  # noqa: ARG002
    ) -> object:
        """Mark a test as a todo placeholder.

        Args:
            fn_or_desc: The test function (when used as a bare decorator)
                or a description string (when called with parentheses).
            description: Description of what needs to be done
                (alternative to positional string).
            name: Optional test name override.
            tags: Optional list of tags for filtering.
        """
        if callable(fn_or_desc):
            _stamp(fn_or_desc, self._attr, "")
            return fn_or_desc
        resolved = fn_or_desc or description or ""

        def decorator(f: _Fn) -> _Fn:
            _stamp(f, self._attr, resolved)
            return f

        return decorator


class _XfailMarker(_Marker, attr="__tryke_xfail__"):
    """Mark a test as expected to fail.

    Can be used as a bare decorator or called with a reason string.

    Example:
        ```python
        @test.xfail
        def known_bug():
            ...

        @test.xfail("upstream issue #42")
        def with_reason():
            ...
        ```
    """

    @overload
    def __call__(self, fn_or_reason: _Fn, /) -> _XfailMarked: ...
    @overload
    def __call__(
        self,
        fn_or_reason: str | None = ...,
        /,
        *,
        reason: str | None = ...,
        name: str | None = ...,
        tags: list[str] | None = ...,
    ) -> Callable[[_Fn], _XfailMarked]: ...

    def __call__(
        self,
        fn_or_reason: _Fn | str | None = None,
        /,
        *,
        reason: str | None = None,
        name: str | None = None,  # noqa: ARG002
        tags: list[str] | None = None,  # noqa: ARG002
    ) -> object:
        """Mark a test as expected to fail.

        Args:
            fn_or_reason: The test function (when used as a bare decorator)
                or a reason string (when called with parentheses).
            reason: Reason the test is expected to fail
                (alternative to positional string).
            name: Optional test name override.
            tags: Optional list of tags for filtering.
        """
        if callable(fn_or_reason):
            _stamp(fn_or_reason, self._attr, "")
            return fn_or_reason
        resolved = fn_or_reason or reason or ""

        def decorator(f: _Fn) -> _Fn:
            _stamp(f, self._attr, resolved)
            return f

        return decorator


class _TestBuilder:
    """Decorator for marking functions as tests.

    Tryke discovers functions decorated with `@test` (or prefixed with
    `test_`) during collection.

    Attributes:
        skip: Skip a test unconditionally.
        todo: Mark a test as a placeholder.
        xfail: Mark a test as expected to fail.

    Example:
        ```python
        from tryke import test

        @test
        def my_test():
            ...

        @test(name="descriptive test name")
        def named():
            ...

        @test(tags=["slow", "network"])
        def tagged():
            ...
        ```
    """

    skip = _SkipMarker()
    todo = _TodoMarker()
    xfail = _XfailMarker()

    @overload
    def __call__(self, fn: _AnyTestFn, /) -> _AnyTestFn: ...
    @overload
    def __call__(self, name: str, /) -> _Decorator: ...
    @overload
    def __call__(
        self,
        fn: None = None,
        /,
        *,
        name: str | None = None,
        tags: list[str] | None = None,
    ) -> _Decorator: ...

    def __call__(
        self,
        fn=None,
        /,
        *,
        name=None,  # noqa: ARG002 - only used by static analysis/test discovery
        tags=None,  # noqa: ARG002 - only used by static analysis/test discovery
    ):
        """Register a function as a test.

        Can be used as a bare decorator (`@test`) or called with keyword
        arguments (`@test(name="...", tags=[...])`) to set metadata.

        Args:
            fn: The test function (when used as a bare decorator).
            name: Optional display name for the test.
            tags: Optional list of tags for filtering with `-m`.
        """
        if callable(fn):
            return fn

        def decorator(f: Callable[[], None]) -> Callable[[], None]:
            return f

        return decorator

    def skip_if(
        self,
        condition: bool,  # noqa: FBT001 - this is clear enough with the method name
        *,
        reason: str = "",
    ) -> _Decorator:
        """Skip a test conditionally, evaluated at import time.

        Args:
            condition: When `True`, the test is skipped.
            reason: Optional reason shown in test output.

        Example:
            ```python
            import sys

            @test.skip_if(sys.platform == "win32", reason="unix only")
            def unix_test():
                ...
            ```
        """

        def decorator(f: _AnyTestFn) -> _AnyTestFn:
            if condition:
                _stamp(f, "__tryke_skip__", reason)
            return f

        return decorator


test = _TestBuilder()
"""The singleton `test` decorator instance.

Use `@test` to mark a function as a test, or access sub-decorators like
`@test.skip`, `@test.todo`, `@test.xfail`, and `@test.skip_if(...)`.
"""


class ExpectationError(AssertionError):
    """Raised when an assertion fails in fatal mode.

    Attributes:
        expected: String describing what was expected.
        received: String describing what was actually received.
    """

    def __init__(self, message: str, *, expected: str, received: str) -> None:
        super().__init__(message)
        self.expected = expected
        self.received = received


_TRYKE_PKG = str(Path(__file__).resolve().parent)


class MatchResult:
    """Result of an assertion.

    By default assertions are **soft** — a failing assertion records the
    failure but does not stop the test. Call `.fatal()` to opt in to
    immediate failure.
    """

    def __init__(self, error: ExpectationError | None) -> None:
        self._error = error

    def __repr__(self) -> str:
        if self._error is None:
            return "MatchResult(ok)"
        return "MatchResult(failed)"

    def fatal(self) -> None:
        """Stop the test immediately if this assertion failed.

        Example:
            ```python
            @test
            def must_pass():
                expect(config).not_.to_be_none().fatal()  # stops here if None
                expect(config.value).to_equal(42)
            ```
        """
        if self._error is not None:
            # Soft-assertion mode recorded this failure already; drop the
            # matching entry so the test runner doesn't report it twice.
            ctx = _soft_ctx.value
            if ctx is not None:
                for i in range(len(ctx.failures) - 1, -1, -1):
                    if ctx.failures[i].error is self._error:
                        del ctx.failures[i]
                        break
            raise self._error


class SoftFailure(NamedTuple):
    """A single soft-assertion failure with its call-site frame."""

    error: ExpectationError
    frame: traceback.FrameSummary | None


class SoftContext:
    def __init__(self) -> None:
        self.failures: list[SoftFailure] = []


class _SoftContextHolder:
    """Mutable holder for the current soft assertion context."""

    value: SoftContext | None = None


_soft_ctx = _SoftContextHolder()


def _set_soft_context(ctx: SoftContext | None) -> None:
    """Set the active soft assertion context."""
    _soft_ctx.value = ctx


def _caller_frame() -> traceback.FrameSummary | None:
    for frame in reversed(traceback.extract_stack()):
        if not str(Path(frame.filename).resolve()).startswith(_TRYKE_PKG):
            return frame
    return None


class Expectation[T]:
    """Chainable assertion wrapper created by [`expect`][tryke.expect.expect].

    Every assertion method returns a [`MatchResult`][tryke.expect.MatchResult].
    Use `.not_` to negate any assertion.

    Example:
        ```pycon
        >>> from tryke import expect
        >>> expect(1 + 1).to_equal(2)
        MatchResult(ok)
        >>> expect(None).not_.to_be_truthy()
        MatchResult(ok)
        ```
    """

    def __init__(self, value: T, *, negated: bool = False) -> None:
        self._value: T = value
        self._negated: bool = negated

    @property
    def not_(self) -> Expectation[T]:
        """Negate the next assertion.

        Example:
            ```pycon
            >>> from tryke import expect
            >>> expect(1).not_.to_equal(2)
            MatchResult(ok)
            >>> expect(None).not_.to_be_truthy()
            MatchResult(ok)
            ```
        """
        return Expectation(self._value, negated=not self._negated)

    def _assert(
        self,
        passed: bool,  # noqa: FBT001 - clear enough
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
            if _soft_ctx.value is not None:
                frame = _caller_frame()
                _soft_ctx.value.failures.append(SoftFailure(err, frame))
                return MatchResult(err)
            raise err
        return MatchResult(None)

    def to_equal(self, other: T) -> MatchResult:
        """Deep equality check (`==`).

        Args:
            other: The value to compare against.

        Example:
            ```pycon
            >>> from tryke import expect
            >>> expect(1 + 1).to_equal(2)
            MatchResult(ok)
            >>> expect([1, 2]).to_equal([1, 2])
            MatchResult(ok)
            ```
        """
        return self._assert(
            self._value == other,
            f"{self._value!r} to equal {other!r}",
            expected=repr(other),
            received=repr(self._value),
        )

    def to_be(self, other: object) -> MatchResult:
        """Identity check (`is`).

        Args:
            other: The object to compare identity against.

        Example:
            ```pycon
            >>> from tryke import expect
            >>> sentinel = object()
            >>> expect(sentinel).to_be(sentinel)
            MatchResult(ok)
            ```
        """
        return self._assert(
            self._value is other,
            f"{self._value!r} to be {other!r}",
            expected=repr(other),
            received=repr(self._value),
        )

    def to_be_truthy(self) -> MatchResult:
        """Assert the value is truthy (`bool(value) is True`).

        Example:
            ```pycon
            >>> from tryke import expect
            >>> expect(1).to_be_truthy()
            MatchResult(ok)
            >>> expect([1]).to_be_truthy()
            MatchResult(ok)
            ```
        """
        return self._assert(
            bool(self._value),
            f"{self._value!r} to be truthy",
            expected="truthy",
            received=repr(self._value),
        )

    def to_be_falsy(self) -> MatchResult:
        """Assert the value is falsy (`bool(value) is False`).

        Example:
            ```pycon
            >>> from tryke import expect
            >>> expect(0).to_be_falsy()
            MatchResult(ok)
            >>> expect("").to_be_falsy()
            MatchResult(ok)
            ```
        """
        return self._assert(
            not bool(self._value),
            f"{self._value!r} to be falsy",
            expected="falsy",
            received=repr(self._value),
        )

    def to_be_none(self) -> MatchResult:
        """Assert the value is `None`.

        Example:
            ```pycon
            >>> from tryke import expect
            >>> expect(None).to_be_none()
            MatchResult(ok)
            >>> expect(42).not_.to_be_none()
            MatchResult(ok)
            ```
        """
        return self._assert(
            self._value is None,
            f"{self._value!r} to be None",
            expected="None",
            received=repr(self._value),
        )

    def to_be_greater_than[C: _SupportsGT](self: Expectation[C], n: C) -> MatchResult:
        """Assert the value is greater than `n`.

        Args:
            n: The value to compare against.

        Example:
            ```pycon
            >>> from tryke import expect
            >>> expect(5).to_be_greater_than(3)
            MatchResult(ok)
            ```
        """
        return self._assert(
            self._value > n,
            f"{self._value!r} to be greater than {n!r}",
            expected=f"> {n!r}",
            received=repr(self._value),
        )

    def to_be_less_than[C: _SupportsLT](self: Expectation[C], n: C) -> MatchResult:
        """Assert the value is less than `n`.

        Args:
            n: The value to compare against.

        Example:
            ```pycon
            >>> from tryke import expect
            >>> expect(3).to_be_less_than(5)
            MatchResult(ok)
            ```
        """
        return self._assert(
            self._value < n,
            f"{self._value!r} to be less than {n!r}",
            expected=f"< {n!r}",
            received=repr(self._value),
        )

    def to_be_greater_than_or_equal[C: _SupportsGE](
        self: Expectation[C], n: C
    ) -> MatchResult:
        """Assert the value is greater than or equal to `n`.

        Args:
            n: The value to compare against.

        Example:
            ```pycon
            >>> from tryke import expect
            >>> expect(5).to_be_greater_than_or_equal(5)
            MatchResult(ok)
            ```
        """
        return self._assert(
            self._value >= n,
            f"{self._value!r} to be greater than or equal to {n!r}",
            expected=f">= {n!r}",
            received=repr(self._value),
        )

    def to_be_less_than_or_equal[C: _SupportsLE](
        self: Expectation[C], n: C
    ) -> MatchResult:
        """Assert the value is less than or equal to `n`.

        Args:
            n: The value to compare against.

        Example:
            ```pycon
            >>> from tryke import expect
            >>> expect(4).to_be_less_than_or_equal(5)
            MatchResult(ok)
            ```
        """
        return self._assert(
            self._value <= n,
            f"{self._value!r} to be less than or equal to {n!r}",
            expected=f"<= {n!r}",
            received=repr(self._value),
        )

    def to_contain[S: _SupportsContains](
        self: Expectation[S], item: object
    ) -> MatchResult:
        """Assert the value contains `item`.

        Works on lists, strings, and any container supporting `in`.

        Args:
            item: The item to search for.

        Example:
            ```pycon
            >>> from tryke import expect
            >>> expect([1, 2, 3]).to_contain(2)
            MatchResult(ok)
            >>> expect("hello world").to_contain("world")
            MatchResult(ok)
            ```
        """
        return self._assert(
            item in self._value,
            f"{self._value!r} to contain {item!r}",
            expected=f"contains {item!r}",
            received=repr(self._value),
        )

    def to_have_length[S: _SupportsLen](self: Expectation[S], n: int) -> MatchResult:
        """Assert the value has length `n`.

        Args:
            n: The expected length.

        Example:
            ```pycon
            >>> from tryke import expect
            >>> expect([1, 2, 3]).to_have_length(3)
            MatchResult(ok)
            >>> expect("hello").to_have_length(5)
            MatchResult(ok)
            ```
        """
        actual = len(self._value)
        return self._assert(
            actual == n,
            f"{self._value!r} to have length {n}, got {actual}",
            expected=f"length {n}",
            received=f"length {actual}",
        )

    def to_match(self, pattern: str) -> MatchResult:
        """Regex match against the string representation of the value.

        Args:
            pattern: A regular expression pattern.

        Example:
            ```pycon
            >>> from tryke import expect
            >>> expect("hello world").to_match(r"hello")
            MatchResult(ok)
            >>> expect("foo123").to_match(r"\\d+")
            MatchResult(ok)
            ```
        """
        return self._assert(
            bool(re.search(pattern, str(self._value))),
            f"{self._value!r} to match pattern {pattern!r}",
            expected=f"matches {pattern!r}",
            received=repr(self._value),
        )

    def to_raise[F: _SupportsCall](
        self: Expectation[F],
        exc_type: type[BaseException] | None = None,
        *,
        match: str | None = None,
    ) -> MatchResult:
        """Assert that a callable raises an exception.

        Wrap the expression in a lambda.

        Args:
            exc_type: Expected exception type, or `None` for any exception.
            match: Regex pattern to match against the exception message.

        Example:
            ```pycon
            >>> from tryke import expect
            >>> expect(lambda: int("abc")).to_raise(ValueError)
            MatchResult(ok)
            >>> expect(lambda: 1 / 0).to_raise(ZeroDivisionError, match="division")
            MatchResult(ok)
            >>> expect(lambda: None).not_.to_raise()
            MatchResult(ok)
            ```
        """
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


def expect[T](
    expr: T,
    name: str | None = None,  # noqa: ARG001 - only used by static analysis/test discovery
) -> Expectation[T]:
    """Create an [`Expectation`][tryke.expect.Expectation] for `expr`.

    Args:
        expr: The value to make assertions on.
        name: Optional label for the assertion (used by the Rust-side
            discovery to extract assertion labels from source code;
            unused at runtime).

    Returns:
        An `Expectation` with chainable assertion methods.

    Example:
        ```pycon
        >>> from tryke import expect
        >>> expect(1 + 1).to_equal(2)
        MatchResult(ok)
        >>> expect("hello").to_contain("ell")
        MatchResult(ok)
        ```
    """
    return Expectation(expr)
