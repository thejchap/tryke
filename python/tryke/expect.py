from collections.abc import Callable


def test(fn: Callable[[], None]) -> Callable[[], None]:
    return fn


class Expectation[T]:
    def to_equal(self, other: T) -> None:
        _ = other


def expect[T](expr: T) -> Expectation[T]:
    _ = expr
    return Expectation()


@test
def test_basic() -> None:
    expect(1).to_equal(1)
