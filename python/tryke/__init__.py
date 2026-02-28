"""Python entrypoint for tryke."""

from collections.abc import Callable


def test(fn: Callable[[], None]) -> Callable[[], None]:
    """Test decorator."""
    return fn


class Expectation[T]:
    """Expectation builder."""

    def to_equal(self, other: T) -> None:
        """Assert self value is equal to other."""
        _ = other


def expect[T](expr: T) -> Expectation[T]:
    """Enter assertion logic."""
    _ = expr
    return Expectation()
