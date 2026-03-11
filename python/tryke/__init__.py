from collections.abc import Generator
from contextlib import contextmanager

from .expect import expect, test


@contextmanager
def describe(name: str) -> Generator[None, None, None]:  # noqa: ARG001
    yield


__all__ = ["describe", "expect", "test"]
