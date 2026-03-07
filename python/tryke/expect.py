from __future__ import annotations

import re
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


class Expectation[T]:
    def __init__(self, value: T, *, negated: bool = False) -> None:
        self._value: T = value
        self._negated: bool = negated

    @property
    def not_(self) -> Expectation[T]:
        return Expectation(self._value, negated=not self._negated)

    def _assert(self, passed: bool, message: str) -> None:  # noqa: FBT001
        ok = (not passed) if self._negated else passed
        if not ok:
            prefix = "expected not " if self._negated else "expected "
            raise AssertionError(prefix + message)

    def to_equal(self, other: T) -> None:
        self._assert(self._value == other, f"{self._value!r} to equal {other!r}")

    def to_be(self, other: object) -> None:
        self._assert(self._value is other, f"{self._value!r} to be {other!r}")

    def to_be_truthy(self) -> None:
        self._assert(bool(self._value), f"{self._value!r} to be truthy")

    def to_be_falsy(self) -> None:
        self._assert(not bool(self._value), f"{self._value!r} to be falsy")

    def to_be_none(self) -> None:
        self._assert(self._value is None, f"{self._value!r} to be None")

    def to_be_greater_than(self, n: T) -> None:
        self._assert(self._value > n, f"{self._value!r} to be greater than {n!r}")  # type: ignore[operator]

    def to_be_less_than(self, n: T) -> None:
        self._assert(self._value < n, f"{self._value!r} to be less than {n!r}")  # type: ignore[operator]

    def to_be_greater_than_or_equal(self, n: T) -> None:
        self._assert(
            self._value >= n,  # type: ignore[operator]
            f"{self._value!r} to be greater than or equal to {n!r}",
        )

    def to_be_less_than_or_equal(self, n: T) -> None:
        self._assert(
            self._value <= n,  # type: ignore[operator]
            f"{self._value!r} to be less than or equal to {n!r}",
        )

    def to_contain(self, item: T) -> None:
        self._assert(item in self._value, f"{self._value!r} to contain {item!r}")  # type: ignore[operator]

    def to_have_length(self, n: int) -> None:
        actual = len(self._value)  # type: ignore[arg-type]
        self._assert(actual == n, f"{self._value!r} to have length {n}, got {actual}")

    def to_match(self, pattern: str) -> None:
        self._assert(
            bool(re.search(pattern, str(self._value))),
            f"{self._value!r} to match pattern {pattern!r}",
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
