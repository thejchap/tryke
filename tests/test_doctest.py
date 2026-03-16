"""Tests that tryke discovers and runs doctests correctly."""

from __future__ import annotations

from tryke import expect, test
from tryke.expect import ExpectationError, MatchResult


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
