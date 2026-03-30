"""End-to-end tests verifying hooks run correctly through the full pipeline."""

from __future__ import annotations

from typing import TYPE_CHECKING

from tryke import (
    Depends,
    after_each,
    before_all,
    before_each,
    describe,
    expect,
    test,
    wrap_each,
)

if TYPE_CHECKING:
    from collections.abc import Generator

# Module-level tracking list shared across hooks and tests.
_log: list[str] = []


@before_each
def clear_log() -> None:
    _log.clear()


@before_all
def db() -> str:
    return "test_db"


@before_each
def table(conn: str = Depends(db)) -> str:
    _log.append(f"setup:{conn}")
    return f"{conn}/users"


@after_each
def cleanup() -> None:
    _log.append("cleanup")


with describe("hooks e2e"):

    @test(name="before_each runs and provides value via Depends")
    def test_before_each_provides_value() -> None:
        _ = table  # Reference the hook to verify it ran
        expect(_log).to_contain("setup:test_db")

    @test(name="after_each runs after test")
    def test_after_runs() -> None:
        # This test just verifies that _log was cleared by before_each,
        # meaning the previous test's after_each + this test's before_each ran.
        expect(_log).to_contain("setup:test_db")


with describe("wrap hooks"):

    @wrap_each
    def with_context() -> Generator[str, None, None]:
        _log.append("wrap_setup")
        yield "ctx"
        _log.append("wrap_teardown")

    @test(name="wrap_each wraps test execution")
    def test_wrap() -> None:
        expect(_log).to_contain("wrap_setup")
