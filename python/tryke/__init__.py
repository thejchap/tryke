from typing import Callable


def test(fn: Callable[[], None]) -> Callable[[], None]:
    return fn
