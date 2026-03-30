"""tryke's public Python API.

The three main exports are:

- [`test`][tryke.expect.test] — decorator for marking test functions
- [`expect`][tryke.expect.expect] — assertion entry point
- [`describe`][tryke.describe] — context manager for grouping tests
"""

from collections.abc import Generator
from contextlib import contextmanager

from .expect import expect, test
from .hooks import (
    Depends,
    after_all,
    after_each,
    before_all,
    before_each,
    wrap_all,
    wrap_each,
)


@contextmanager
def describe(
    name: str,  # noqa: ARG001 - only used by static analysis/test discovery
) -> Generator[None, None, None]:
    """Group tests visually in output.

    The describe name is used as a prefix in test names during reporting.

    Args:
        name: The group name shown in test output.

    Example:
        ```python
        from tryke import describe, expect, test

        with describe("math"):
            @test
            def addition():
                expect(1 + 1).to_equal(2)

            @test
            def subtraction():
                expect(3 - 1).to_equal(2)
        ```
    """
    yield


__all__ = [
    "Depends",
    "after_all",
    "after_each",
    "before_all",
    "before_each",
    "describe",
    "expect",
    "test",
    "wrap_all",
    "wrap_each",
]
