# ruff: noqa: B008, PT028 — Depends() in defaults is the intended API pattern.
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

    @test(name="before_each runs independently per test")
    def test_after_runs() -> None:
        # _log was cleared by clear_log() in before_each for this test,
        # so this only verifies that the per-test setup hook ran.
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


# ---------------------------------------------------------------------------
# before_all instance reuse
# ---------------------------------------------------------------------------

# Track how many times the expensive resource is created.
_expensive_call_count = 0


@before_all
def expensive_resource() -> dict[str, int]:
    """Simulates an expensive setup that should only happen once."""
    global _expensive_call_count  # noqa: PLW0603
    _expensive_call_count += 1
    return {"created": _expensive_call_count, "value": 42}


with describe("before_all reuse"):

    @test(name="first test receives the before_all instance")
    def test_reuse_first(
        res: dict[str, int] = Depends(expensive_resource),
    ) -> None:
        expect(res["value"]).to_equal(42)
        # Created exactly once so far
        expect(res["created"]).to_equal(1)
        expect(_expensive_call_count).to_equal(1)

    @test(name="second test gets the same cached instance")
    def test_reuse_second(
        res: dict[str, int] = Depends(expensive_resource),
    ) -> None:
        # Still the same instance — before_all was NOT called again
        expect(res["created"]).to_equal(1)
        expect(_expensive_call_count).to_equal(1)

    @test(name="third test confirms no additional calls")
    def test_reuse_third(
        res: dict[str, int] = Depends(expensive_resource),
    ) -> None:
        expect(res["created"]).to_equal(1)
        expect(_expensive_call_count).to_equal(1)


# ---------------------------------------------------------------------------
# Composability via Depends chains
# ---------------------------------------------------------------------------


@before_all
def app_config() -> dict[str, str]:
    return {"db_url": "sqlite:///:memory:", "cache_url": "redis://localhost"}


@before_all
def database(cfg: dict[str, str] = Depends(app_config)) -> str:
    return f"Database({cfg['db_url']})"


@before_all
def cache(cfg: dict[str, str] = Depends(app_config)) -> str:
    return f"Cache({cfg['cache_url']})"


@before_each
def user_service(
    db_conn: str = Depends(database),
    cache_conn: str = Depends(cache),
) -> str:
    return f"UserService({db_conn}, {cache_conn})"


with describe("composability"):

    @test(name="test receives fully resolved dependency chain")
    def test_composed_service(
        svc: str = Depends(user_service),
        cfg: dict[str, str] = Depends(app_config),
        db_conn: str = Depends(database),
        cache_conn: str = Depends(cache),
    ) -> None:
        # Leaf dependency
        expect(cfg).to_equal(
            {"db_url": "sqlite:///:memory:", "cache_url": "redis://localhost"}
        )
        # Mid-level: each resolved with config injected
        expect(db_conn).to_equal("Database(sqlite:///:memory:)")
        expect(cache_conn).to_equal("Cache(redis://localhost)")
        # Top-level: composed from db + cache
        expect(svc).to_equal(
            "UserService(Database(sqlite:///:memory:), Cache(redis://localhost))"
        )

    @test(name="before_each produces fresh value each test, before_all is reused")
    def test_fresh_each_reused_all(
        svc: str = Depends(user_service),
    ) -> None:
        # user_service is before_each — resolved fresh.
        # database/cache are before_all — same cached values.
        expect(svc).to_equal(
            "UserService(Database(sqlite:///:memory:), Cache(redis://localhost))"
        )


@before_all
def base_url() -> str:
    return "http://localhost:8000"


with describe("composability > nested describe"):

    @before_each
    def auth_header(url: str = Depends(base_url)) -> dict[str, str]:
        return {"Authorization": f"Bearer token-for-{url}"}

    @test(name="describe-scoped hook depends on module-scoped before_all")
    def test_nested_depends(
        header: dict[str, str] = Depends(auth_header),
    ) -> None:
        expect(header).to_equal(
            {"Authorization": "Bearer token-for-http://localhost:8000"}
        )
