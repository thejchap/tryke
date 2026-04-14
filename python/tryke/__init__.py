"""tryke's public Python API.

The main exports are:

- [`test`][tryke.expect.test] — decorator for marking test functions
- [`expect`][tryke.expect.expect] — assertion entry point
- [`describe`][tryke.describe] — context manager for grouping tests
- [`fixture`][tryke.hooks.fixture] — decorator for setup/teardown fixtures
- [`Depends`][tryke.hooks.Depends] — wire fixture values into signatures
"""

from collections.abc import Generator
from contextlib import contextmanager

from .expect import expect, test
from .hooks import Depends, fixture


@contextmanager
def describe(name: str) -> Generator[None, None, None]:
    """Group tests visually in output.

    The describe name is used as a prefix in test names during reporting.
    The name is inspected by tryke's static discovery, so it must be a
    string literal when used as ``with describe("..."):``.

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
    # `name` is referenced here purely to satisfy the "used" requirement
    # without a lint suppression; static discovery is what actually reads it.
    _ = name
    yield


__all__ = [
    "Depends",
    "describe",
    "expect",
    "fixture",
    "test",
]
