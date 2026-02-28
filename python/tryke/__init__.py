from typing import Callable


def test(fn: Callable[[], None]) -> Callable[[], None]:
    return fn


class Expectation[T]:
    def to_equal(self, other: T) -> None:
        pass


def expect[T](expr: T) -> Expectation[T]:
    return Expectation()
