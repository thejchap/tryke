"""Tests for the fixture module: @fixture, Depends(), resolver, and e2e."""

from __future__ import annotations

import asyncio
from typing import TYPE_CHECKING, Annotated, assert_type

import tryke
from tryke import Depends, describe, expect, fixture, test
from tryke.hooks import (
    CyclicDependencyError,
    DependencyResolver,
    HookExecutor,
    _Depends,
    _depends_from_annotation,
    _fixture_per,
)

if TYPE_CHECKING:
    from collections.abc import AsyncGenerator, Generator


with describe("@fixture decorator"):

    @test(name="bare @fixture stamps per='test'")
    def test_bare_fixture_stamps() -> None:
        @fixture
        def setup() -> int:
            return 42

        expect(_fixture_per(setup), "bare @fixture stamps per=test").to_equal("test")
        expect(setup(), "fixture body still callable").to_equal(42)

    @test(name="@fixture() with no kwargs stamps per='test'")
    def test_call_form_default() -> None:
        @fixture()
        def setup() -> int:
            return 1

        expect(_fixture_per(setup), "@fixture() defaults to per=test").to_equal("test")
        expect(setup(), "fixture body still callable").to_equal(1)

    @test(name="@fixture(per='scope') stamps per='scope'")
    def test_scope_fixture_stamps() -> None:
        @fixture(per="scope")
        def db() -> str:
            return "conn"

        expect(_fixture_per(db), "per='scope' stamped").to_equal("scope")
        expect(db(), "fixture body still callable").to_equal("conn")

    @test(name="@fixture(per='test') explicit form stamps per='test'")
    def test_explicit_test_form() -> None:
        @fixture(per="test")
        def setup() -> int:
            return 1

        expect(_fixture_per(setup), "explicit per='test' stamped").to_equal("test")

    @test(name="decorated function is unchanged")
    def test_function_unchanged() -> None:
        def original() -> int:
            return 99

        decorated = fixture(original)
        expect(decorated, "fixture returns same function object").to_be(original)
        expect(decorated(), "original body preserved").to_equal(99)

    @test(name="_fixture_per returns None for undecorated function")
    def test_fixture_per_none_for_plain() -> None:
        def plain() -> int:
            return 1

        expect(_fixture_per(plain), "plain fn has no per marker").to_be_none()


with describe("Depends"):

    @test(name="Depends returns _Depends instance")
    def test_depends_returns_sentinel() -> None:
        def my_hook() -> int:
            return 42

        dep = _Depends(my_hook)
        expect(isinstance(dep, _Depends), "Depends() returns a _Depends").to_be_truthy()
        expect(dep.dependency, "stores hook reference").to_be(my_hook)

    @test(name="Depends stores the dependency callable")
    def test_depends_stores_callable() -> None:
        def hook_a() -> str:
            return "a"

        def hook_b() -> str:
            return "b"

        dep_a = _Depends(hook_a)
        dep_b = _Depends(hook_b)
        expect(dep_a.dependency, "dep_a wraps hook_a").to_be(hook_a)
        expect(dep_b.dependency, "dep_b wraps hook_b").to_be(hook_b)

    @test(name="_Depends is frozen")
    def test_depends_frozen() -> None:
        def my_hook() -> int:
            return 1

        dep = Depends(my_hook)
        expect(
            lambda: setattr(dep, "dependency", None),
            "mutating frozen _Depends raises",
        ).to_raise(AttributeError)


with describe("public exports"):

    @test(name="fixture and Depends are exported from tryke package")
    def test_exports() -> None:
        expect(hasattr(tryke, "fixture"), "tryke.fixture exported").to_be_truthy()
        expect(hasattr(tryke, "Depends"), "tryke.Depends exported").to_be_truthy()


with describe("DependencyResolver"):

    @test(name="resolves a simple Depends chain")
    def test_resolve_simple() -> None:
        @fixture(per="scope")
        def db() -> str:
            return "conn"

        @fixture(per="scope")
        def table(conn: str = Depends(db)) -> str:
            return f"{conn}/table"

        resolver = DependencyResolver()
        result = resolver.resolve(table)
        expect(result, "resolved kwargs for `table`").to_equal({"conn": "conn"})

    @test(name="caches resolved values per function identity")
    def test_caching() -> None:
        call_count = 0

        @fixture
        def counter() -> int:
            nonlocal call_count
            call_count += 1
            return call_count

        @fixture
        def user_a(c: int = Depends(counter)) -> str:
            return f"a:{c}"

        @fixture
        def user_b(c: int = Depends(counter)) -> str:
            return f"b:{c}"

        resolver = DependencyResolver()
        a = resolver.resolve(user_a)
        b = resolver.resolve(user_b)
        expect(a["c"], "user_a sees counter=1").to_equal(1)
        expect(b["c"], "user_b sees cached counter=1").to_equal(1)
        expect(call_count, "counter only ran once").to_equal(1)

    @test(name="detects dependency cycles")
    def test_cycle_detection() -> None:
        @fixture
        def hook_a(_b: str = Depends(lambda: "")) -> str:
            return "a"

        @fixture
        def hook_b(_a: str = Depends(hook_a)) -> str:
            return "b"

        # Manually wire the cycle: hook_a depends on hook_b
        hook_a.__defaults__ = (Depends(hook_b),)

        resolver = DependencyResolver()
        expect(lambda: resolver.resolve(hook_a), "cyclic dependency raises").to_raise(
            CyclicDependencyError
        )

    @test(name="resolves generator fixtures via next()")
    def test_generator_resolution() -> None:
        teardown_ran = False

        @fixture
        def with_resource() -> Generator[str, None, None]:
            nonlocal teardown_ran
            yield "resource"
            teardown_ran = True

        resolver = DependencyResolver()
        value = resolver.resolve_hook(with_resource)
        expect(value, "generator yielded value").to_equal("resource")
        expect(teardown_ran, "teardown not run before finalize").to_be_falsy()

        resolver.teardown_test_generators()
        expect(teardown_ran, "teardown ran after finalize").to_be_truthy()

    @test(name="clear_test_cache resets per-test values but keeps per-scope")
    def test_clear_test_cache_preserves_scope() -> None:
        test_count = 0
        scope_count = 0

        @fixture
        def per_test() -> int:
            nonlocal test_count
            test_count += 1
            return test_count

        @fixture(per="scope")
        def per_scope() -> int:
            nonlocal scope_count
            scope_count += 1
            return scope_count

        resolver = DependencyResolver()
        expect(resolver.resolve_hook(per_test), "per-test first call").to_equal(1)
        expect(resolver.resolve_hook(per_scope), "per-scope first call").to_equal(1)

        resolver.clear_test_cache()
        # per-test resets; per-scope value preserved
        expect(
            resolver.resolve_hook(per_test), "per-test re-runs after clear"
        ).to_equal(2)
        expect(
            resolver.resolve_hook(per_scope), "per-scope preserved across clear"
        ).to_equal(1)


# Module-level helpers used by the "Depends via Annotated" tests below.
# These are deliberately *not* decorated with ``@fixture`` — they exist
# purely to give the resolver something to chase, and module-level
# ``per="scope"`` fixtures would force the whole file onto a single
# worker. ``DependencyResolver`` defaults undecorated callables to
# ``per="test"``. They're at module scope (not inside a test function)
# so PEP-563 string annotations resolve via ``typing.get_type_hints``.
def _annotated_db() -> str:
    return "conn"


def _annotated_cache() -> int:
    return 7


def _annotated_table(conn: Annotated[str, Depends(_annotated_db)]) -> str:
    return f"{conn}/table"


with describe("Depends via Annotated"):

    @test(name="_depends_from_annotation extracts _Depends from Annotated metadata")
    def test_extract_from_annotated_metadata() -> None:
        def hook() -> int:
            return 1

        annotation = Annotated[int, Depends(hook)]
        # ``_Depends`` is a frozen dataclass, so equality compares the
        # wrapped callable by identity — no need to unwrap manually.
        expect(
            _depends_from_annotation(annotation),
            "extracts the _Depends marker",
        ).to_equal(_Depends(hook))

    @test(name="_depends_from_annotation returns None for plain type")
    def test_extract_from_plain_type() -> None:
        expect(_depends_from_annotation(int), "no metadata on bare int").to_be_none()
        expect(_depends_from_annotation(None), "None has no metadata").to_be_none()
        expect(
            _depends_from_annotation(Annotated[int, "no depends here"]),
            "Annotated without _Depends metadata",
        ).to_be_none()

    @test(name="_depends_from_annotation picks the first _Depends in metadata")
    def test_extract_picks_first_depends() -> None:
        def hook_a() -> int:
            return 1

        def hook_b() -> int:
            return 2

        annotation = Annotated[int, Depends(hook_a), "extra", Depends(hook_b)]
        expect(
            _depends_from_annotation(annotation),
            "first _Depends wins",
        ).to_equal(_Depends(hook_a))

    @test(name="resolver resolves Annotated[T, Depends(...)] params")
    def test_resolver_handles_annotated_form() -> None:
        resolver = DependencyResolver()
        result = resolver.resolve(_annotated_table)
        expect(result, "Annotated metadata is resolved").to_equal({"conn": "conn"})

    @test(name="default-value Depends takes precedence over Annotated metadata")
    def test_default_form_wins_over_annotated() -> None:
        # Locally defined for runtime injection; closure types are fine
        # because both Depends markers are runtime objects, not strings.
        @fixture(per="scope")
        def primary() -> str:
            return "primary"

        @fixture(per="scope")
        def secondary() -> str:
            return "secondary"

        @fixture
        def consumer(
            x: Annotated[str, Depends(secondary)] = Depends(primary),
        ) -> str:
            return x

        resolver = DependencyResolver()
        result = resolver.resolve(consumer)
        expect(result, "default Depends wins").to_equal({"x": "primary"})

    @test(name="resolver mixes default and Annotated Depends in same function")
    def test_mixed_default_and_annotated() -> None:
        resolver = DependencyResolver()

        # Use module-level fixtures so get_type_hints can resolve the
        # Annotated form's string annotation against module globals.
        def consumer(
            conn: Annotated[str, Depends(_annotated_db)],
            count: int = Depends(_annotated_cache),
        ) -> tuple[str, int]:
            return conn, count

        result = resolver.resolve(consumer)
        expect(result, "both forms resolved together").to_equal(
            {"conn": "conn", "count": 7}
        )

    @test(name="resolver tolerates unresolvable annotations on local fixtures")
    def test_resolver_ignores_unresolvable_annotation() -> None:
        # Annotation references a local closure type that
        # ``typing.get_type_hints`` cannot resolve under PEP 563. The
        # resolver must fall back to the default-value form rather than
        # raising NameError.
        class Local:
            pass

        @fixture(per="scope")
        def db() -> Local:
            return Local()

        @fixture
        def consumer(value: Local = Depends(db)) -> Local:
            return value

        resolver = DependencyResolver()
        result = resolver.resolve(consumer)
        expect(set(result.keys()), "default-form Depends still resolves").to_equal(
            {"value"}
        )
        expect(
            isinstance(result["value"], Local), "value has the right type"
        ).to_be_truthy()


with describe("HookExecutor basics"):

    @test(name="runs per-test fixture before test")
    def test_per_test_setup_runs() -> None:
        log: list[str] = []

        @fixture
        def setup() -> None:
            log.append("setup")

        def my_test() -> None:
            log.append("test")

        executor = HookExecutor()
        executor.register_fixture(setup, groups=[])
        executor.run_test(my_test, groups=[])
        expect(log, "setup runs before test").to_equal(["setup", "test"])

    @test(name="generator fixture's post-yield runs after test")
    def test_generator_teardown_runs() -> None:
        log: list[str] = []

        @fixture
        def wrapper() -> Generator[None, None, None]:
            log.append("setup")
            yield
            log.append("teardown")

        def my_test() -> None:
            log.append("test")

        executor = HookExecutor()
        executor.register_fixture(wrapper, groups=[])
        executor.run_test(my_test, groups=[])
        expect(log, "setup/test/teardown order").to_equal(["setup", "test", "teardown"])

    @test(name="outer scope fixtures wrap inner scope fixtures")
    def test_scope_nesting() -> None:
        log: list[str] = []

        @fixture
        def outer_setup() -> None:
            log.append("outer")

        @fixture
        def inner_setup() -> None:
            log.append("inner")

        def my_test() -> None:
            log.append("test")

        executor = HookExecutor()
        executor.register_fixture(outer_setup, groups=[])
        executor.register_fixture(inner_setup, groups=["users"])
        executor.run_test(my_test, groups=["users"])
        expect(log, "outer wraps inner wraps test").to_equal(["outer", "inner", "test"])

    @test(name="generator fixtures tear down in reverse definition order (LIFO)")
    def test_teardown_lifo() -> None:
        log: list[str] = []

        @fixture
        def first() -> Generator[None, None, None]:
            yield
            log.append("first")

        @fixture
        def second() -> Generator[None, None, None]:
            yield
            log.append("second")

        def my_test() -> None:
            log.append("test")

        executor = HookExecutor()
        executor.register_fixture(first, groups=[], line_number=1)
        executor.register_fixture(second, groups=[], line_number=2)
        executor.run_test(my_test, groups=[])
        expect(log, "teardown order is LIFO").to_equal(["test", "second", "first"])

    @test(name="test can receive values via Depends")
    def test_depends_in_test() -> None:
        @fixture
        def db() -> str:
            return "conn"

        received: dict[str, str] = {}

        def my_test(conn: str = Depends(db)) -> None:
            received["conn"] = conn

        executor = HookExecutor()
        executor.register_fixture(db, groups=[])
        executor.run_test(my_test, groups=[])
        expect(received["conn"], "test received fixture value").to_equal("conn")


with describe("per='scope' sharing semantics"):
    # These tests pin the same-worker sharing semantics documented in
    # docs/concepts/concurrency.md. per='scope' values are cached by
    # function identity per HookExecutor, and two tests on the same
    # executor share the object by reference. This is intentional — it's
    # what makes scope-level fixtures a once-per-scope cache.

    @test(name="per='scope' fixture runs exactly once across tests")
    def test_scope_fixture_runs_once() -> None:
        call_count = 0

        @fixture(per="scope")
        def setup() -> str:
            nonlocal call_count
            call_count += 1
            return "resource"

        def test_a() -> None:
            pass

        def test_b() -> None:
            pass

        executor = HookExecutor()
        executor.register_fixture(setup, groups=[])
        executor.run_test(test_a, groups=[])
        executor.run_test(test_b, groups=[])
        expect(call_count, "per='scope' setup ran once across 2 tests").to_equal(1)

    @test(name="per='scope' value is shared by reference across tests")
    def test_scope_shared_by_reference() -> None:
        @fixture(per="scope")
        def shared_config() -> dict[str, str]:
            return {"env": "test", "mutations": ""}

        def mutating_test(
            cfg: dict[str, str] = Depends(shared_config),
        ) -> None:
            cfg["mutations"] += "a"

        seen: list[str] = []

        def observing_test(
            cfg: dict[str, str] = Depends(shared_config),
        ) -> None:
            seen.append(cfg["mutations"])

        executor = HookExecutor()
        executor.register_fixture(shared_config, groups=[])
        executor.run_test(mutating_test, groups=[])
        executor.run_test(observing_test, groups=[])
        # The second test sees the first test's mutation. If this
        # assertion ever fails, either (a) we changed scoping semantics
        # (update docs/concepts/concurrency.md), or (b) we added
        # defensive copying (delete this test and replace with one
        # asserting non-observability).
        expect(seen, "second test saw first test's mutation").to_equal(["a"])
        executor.finalize()

    @test(name="per='scope' generator teardown runs on finalize, not per test")
    def test_scope_generator_teardown_on_finalize() -> None:
        log: list[str] = []

        @fixture(per="scope")
        def wrapper() -> Generator[str, None, None]:
            log.append("setup")
            yield "ctx"
            log.append("teardown")

        def test_a() -> None:
            log.append("test_a")

        def test_b() -> None:
            log.append("test_b")

        executor = HookExecutor()
        executor.register_fixture(wrapper, groups=[])
        executor.run_test(test_a, groups=[])
        executor.run_test(test_b, groups=[])
        expect(log, "no teardown between tests").to_equal(["setup", "test_a", "test_b"])

        executor.finalize()
        expect(log, "teardown runs on finalize").to_equal(
            ["setup", "test_a", "test_b", "teardown"]
        )

    @test(name="multiple per='scope' teardowns run LIFO on finalize")
    def test_multiple_scope_teardown_lifo() -> None:
        log: list[str] = []

        @fixture(per="scope")
        def wrap_a() -> Generator[None, None, None]:
            log.append("a_setup")
            yield
            log.append("a_teardown")

        @fixture(per="scope")
        def wrap_b() -> Generator[None, None, None]:
            log.append("b_setup")
            yield
            log.append("b_teardown")

        def my_test() -> None:
            log.append("test")

        executor = HookExecutor()
        executor.register_fixture(wrap_a, groups=[], line_number=1)
        executor.register_fixture(wrap_b, groups=[], line_number=2)
        executor.run_test(my_test, groups=[])
        expect(log, "both setups ran before test").to_equal(
            ["a_setup", "b_setup", "test"]
        )

        executor.finalize()
        expect(log, "teardown is LIFO on finalize").to_equal(
            ["a_setup", "b_setup", "test", "b_teardown", "a_teardown"]
        )

    @test(name="finalize only tears down per='scope' fixtures that actually ran")
    def test_finalize_skips_unvisited_scopes() -> None:
        log: list[str] = []

        @fixture(per="scope")
        def users_setup() -> Generator[None, None, None]:
            log.append("users_setup")
            yield
            log.append("users_teardown")

        @fixture(per="scope")
        def admin_setup() -> Generator[None, None, None]:
            log.append("admin_setup")
            yield
            log.append("admin_teardown")

        def my_test() -> None:
            log.append("test")

        executor = HookExecutor()
        executor.register_fixture(users_setup, groups=["users"], line_number=1)
        executor.register_fixture(admin_setup, groups=["admin"], line_number=2)
        # Only run a test in the "users" scope.
        executor.run_test(my_test, groups=["users"])
        expect(log, "only the used scope's setup ran").to_equal(["users_setup", "test"])

        executor.finalize()
        expect(log, "only used scope's teardown ran").to_equal(
            ["users_setup", "test", "users_teardown"]
        )


with describe("error handling"):

    @test(name="per-test fixture setup failure propagates")
    def test_setup_failure() -> None:
        @fixture
        def bad_setup() -> None:
            msg = "setup failed"
            raise RuntimeError(msg)

        log: list[str] = []

        def my_test() -> None:
            log.append("test")

        executor = HookExecutor()
        executor.register_fixture(bad_setup, groups=[])
        expect(
            lambda: executor.run_test(my_test, groups=[]),
            "setup failure propagates from run_test",
        ).to_raise(RuntimeError)
        # Test should NOT have run.
        expect(log, "test body skipped when setup fails").to_have_length(0)

    @test(name="generator teardown still runs when test fails")
    def test_teardown_on_failure() -> None:
        log: list[str] = []

        @fixture
        def wrapper() -> Generator[None, None, None]:
            log.append("setup")
            yield
            log.append("teardown")

        def failing_test() -> None:
            log.append("test")
            msg = "test failed"
            raise RuntimeError(msg)

        executor = HookExecutor()
        executor.register_fixture(wrapper, groups=[])
        expect(
            lambda: executor.run_test(failing_test, groups=[]),
            "failing test propagates",
        ).to_raise(RuntimeError)
        expect(log, "teardown ran despite test failure").to_contain("teardown")


with describe("generator lifecycle"):

    @test(name="multi-yield generator raises RuntimeError on teardown")
    def test_multi_yield_raises() -> None:
        @fixture
        def bad_hook() -> Generator[str, None, None]:
            yield "first"
            yield "second"

        resolver = DependencyResolver()
        resolver.resolve_hook(bad_hook)
        expect(
            resolver.teardown_test_generators,
            "multi-yield generator rejected at teardown",
        ).to_raise(RuntimeError, match="yielded more than once")

    @test(name="gen.close() is called even on teardown error")
    def test_gen_close_called() -> None:
        close_called = False

        @fixture
        def tracked_hook() -> Generator[str, None, None]:
            nonlocal close_called
            try:
                yield "value"
            finally:
                close_called = True

        resolver = DependencyResolver()
        resolver.resolve_hook(tracked_hook)
        resolver.teardown_test_generators()
        expect(close_called, "gen.close() reached finally block").to_be_truthy()


with describe("async generator lifecycle"):

    @test(name="async generator teardown runs post-yield code")
    def test_async_gen_teardown() -> None:
        teardown_ran = False

        @fixture
        async def async_resource() -> AsyncGenerator[str, None]:
            nonlocal teardown_ran
            yield "async_val"
            teardown_ran = True

        resolver = DependencyResolver()
        value = resolver.resolve_hook(async_resource)
        expect(value, "async generator yielded value").to_equal("async_val")
        expect(teardown_ran, "teardown not run before finalize").to_be_falsy()

        resolver.teardown_test_generators()
        expect(teardown_ran, "teardown ran after finalize").to_be_truthy()


with describe("async fixtures in HookExecutor"):

    @test(name="async per-test fixture runs before test")
    def test_async_setup_runs() -> None:
        log: list[str] = []

        @fixture
        async def setup() -> None:
            log.append("async_setup")

        def my_test() -> None:
            log.append("test")

        executor = HookExecutor()
        executor.register_fixture(setup, groups=[])
        executor.run_test(my_test, groups=[])
        expect(log, "async setup ran before test").to_equal(["async_setup", "test"])

    @test(name="async per='scope' fixture runs once across tests")
    def test_async_scope_runs_once() -> None:
        call_count = 0

        @fixture(per="scope")
        async def setup() -> str:
            nonlocal call_count
            call_count += 1
            return "resource"

        def test_a() -> None:
            pass

        def test_b() -> None:
            pass

        executor = HookExecutor()
        executor.register_fixture(setup, groups=[])
        executor.run_test(test_a, groups=[])
        executor.run_test(test_b, groups=[])
        expect(call_count, "async per='scope' setup ran once across 2 tests").to_equal(
            1
        )

    @test(name="async per='scope' generator teardown runs on finalize")
    def test_async_scope_generator_finalize() -> None:
        log: list[str] = []

        @fixture(per="scope")
        async def wrapper() -> AsyncGenerator[None, None]:
            log.append("setup")
            yield
            log.append("teardown")

        def test_a() -> None:
            log.append("test_a")

        def test_b() -> None:
            log.append("test_b")

        executor = HookExecutor()
        executor.register_fixture(wrapper, groups=[])
        executor.run_test(test_a, groups=[])
        executor.run_test(test_b, groups=[])
        expect(log, "async scope teardown deferred until finalize").to_equal(
            ["setup", "test_a", "test_b"]
        )

        executor.finalize()
        expect(log, "async scope teardown ran on finalize").to_equal(
            ["setup", "test_a", "test_b", "teardown"]
        )

    @test(name="async per-test generator wraps test execution")
    def test_async_generator_wraps() -> None:
        log: list[str] = []

        @fixture
        async def wrapper() -> AsyncGenerator[None, None]:
            log.append("async_setup")
            yield
            log.append("async_teardown")

        def my_test() -> None:
            log.append("test")

        executor = HookExecutor()
        executor.register_fixture(wrapper, groups=[])
        executor.run_test(my_test, groups=[])
        expect(log, "async per-test generator wraps test execution").to_equal(
            ["async_setup", "test", "async_teardown"]
        )

    @test(name="async test function runs correctly")
    def test_async_test_fn_runs() -> None:
        log: list[str] = []

        async def my_test() -> None:
            log.append("async_test")

        executor = HookExecutor()
        executor.run_test(my_test, groups=[])
        expect(log, "async test fn ran").to_equal(["async_test"])

    @test(name="async fixture provides value to async test via Depends")
    def test_async_depends_in_async_test() -> None:
        @fixture
        async def db() -> str:
            return "async_conn"

        received: dict[str, str] = {}

        async def my_test(conn: str = Depends(db)) -> None:
            received["conn"] = conn

        executor = HookExecutor()
        executor.register_fixture(db, groups=[])
        executor.run_test(my_test, groups=[])
        expect(
            received["conn"], "async fixture value reached test via Depends"
        ).to_equal("async_conn")

    @test(name="async Depends chain: async fixture depending on async fixture")
    def test_async_depends_chain() -> None:
        @fixture
        async def db() -> str:
            return "conn"

        @fixture
        async def table(conn: str = Depends(db)) -> str:
            return f"{conn}/table"

        received: dict[str, str] = {}

        async def my_test(t: str = Depends(table)) -> None:
            received["t"] = t

        executor = HookExecutor()
        executor.register_fixture(db, groups=[])
        executor.register_fixture(table, groups=[])
        executor.run_test(my_test, groups=[])
        expect(received["t"], "async Depends chain resolved conn/table").to_equal(
            "conn/table"
        )

    @test(name="async generator teardown runs when async test fails")
    def test_async_teardown_on_failure() -> None:
        log: list[str] = []

        @fixture
        async def wrapper() -> AsyncGenerator[None, None]:
            log.append("setup")
            yield
            log.append("teardown")

        async def failing_test() -> None:
            log.append("test")
            msg = "boom"
            raise RuntimeError(msg)

        executor = HookExecutor()
        executor.register_fixture(wrapper, groups=[])
        expect(
            lambda: executor.run_test(failing_test, groups=[]),
            "async test failure propagates",
        ).to_raise(RuntimeError)
        expect(log, "async teardown ran despite test failure").to_contain("teardown")

    @test(name="async fixture and async test share one event loop")
    def test_async_fixture_and_test_share_loop() -> None:
        @fixture
        async def loop_bound_resource() -> AsyncGenerator[asyncio.Future[int], None]:
            # Future is bound to whatever loop is running right now.
            fut: asyncio.Future[int] = asyncio.get_running_loop().create_future()
            fut.set_result(7)
            yield fut

        received: dict[str, int] = {}

        async def my_test(
            fut: asyncio.Future[int] = Depends(loop_bound_resource),
        ) -> None:
            # If the test's loop differs from the fixture's loop, this
            # raises "got Future <...> attached to a different loop".
            received["value"] = await fut

        executor = HookExecutor()
        executor.register_fixture(loop_bound_resource, groups=[])
        executor.run_test(my_test, groups=[])
        executor.finalize()
        expect(received["value"], "shared loop awaited fixture future").to_equal(7)

    @test(name="async generator aclose is called on teardown")
    def test_async_gen_aclose_called() -> None:
        close_called = False

        @fixture
        async def tracked_hook() -> AsyncGenerator[str, None]:
            nonlocal close_called
            try:
                yield "value"
            finally:
                close_called = True

        resolver = DependencyResolver()
        resolver.resolve_hook(tracked_hook)
        resolver.teardown_test_generators()
        expect(close_called, "async gen aclose reached finally block").to_be_truthy()


with describe("Depends typing"):

    @test(name="assert_type validates Depends return type for plain function")
    def test_depends_type_plain() -> None:
        @fixture(per="scope")
        def db() -> str:
            return "conn"

        val = Depends(db)
        assert_type(val, str)

    @test(name="assert_type validates Depends return type for generator")
    def test_depends_type_generator() -> None:
        @fixture
        def resource() -> Generator[int, None, None]:
            yield 42

        val = Depends(resource)
        assert_type(val, int)

    @test(name="assert_type validates Depends return type for async coroutine")
    def test_depends_type_async_coroutine() -> None:
        @fixture
        async def resource() -> str:
            return "async_val"

        val = Depends(resource)
        assert_type(val, str)

    @test(name="assert_type validates Depends return type for async generator")
    def test_depends_type_async_generator() -> None:
        @fixture
        async def resource() -> AsyncGenerator[int, None]:
            yield 42

        val = Depends(resource)
        assert_type(val, int)


# ---------------------------------------------------------------------------
# E2E: fixtures through the full pipeline
# ---------------------------------------------------------------------------

# Module-level tracking list shared across fixtures and tests.
_log: list[str] = []


@fixture
def clear_log() -> None:
    _log.clear()


@fixture(per="scope")
def db_conn() -> str:
    return "test_db"


@fixture
def table(conn: str = Depends(db_conn)) -> str:
    _log.append(f"setup:{conn}")
    return f"{conn}/users"


with describe("fixtures e2e"):

    @test(name="per-test fixture runs and provides value via Depends")
    def test_per_test_provides_value() -> None:
        _ = table  # Reference the fixture to verify it ran
        expect(_log, "per-test setup ran for first test").to_contain("setup:test_db")

    @test(name="per-test fixture runs independently per test")
    def test_runs_independently() -> None:
        # _log was cleared by clear_log in the previous test's context,
        # so this only verifies that the per-test setup fixture ran.
        expect(_log, "per-test setup ran again for next test").to_contain(
            "setup:test_db"
        )


with describe("per='scope' instance reuse"):
    # Track how many times the expensive resource is created.
    _expensive_call_count: list[int] = [0]

    @fixture(per="scope")
    def expensive_resource() -> dict[str, int]:
        """Simulates an expensive setup that should only happen once."""
        _expensive_call_count[0] += 1
        return {"created": _expensive_call_count[0], "value": 42}

    @test(name="first test receives the per='scope' instance")
    def test_reuse_first(
        res: dict[str, int] = Depends(expensive_resource),
    ) -> None:
        expect(res["value"], "scope fixture provided value").to_equal(42)
        expect(res["created"], "scope fixture created on first use").to_equal(1)
        expect(_expensive_call_count[0], "scope fixture ran exactly once").to_equal(1)

    @test(name="second test gets the same cached instance")
    def test_reuse_second(
        res: dict[str, int] = Depends(expensive_resource),
    ) -> None:
        expect(res["created"], "second test sees cached instance").to_equal(1)
        expect(_expensive_call_count[0], "scope fixture not re-run").to_equal(1)


# ---------------------------------------------------------------------------
# Composability via Depends chains
# ---------------------------------------------------------------------------


@fixture(per="scope")
def app_config() -> dict[str, str]:
    return {"db_url": "sqlite:///:memory:", "cache_url": "redis://localhost"}


@fixture(per="scope")
def database(cfg: dict[str, str] = Depends(app_config)) -> str:
    return f"Database({cfg['db_url']})"


@fixture(per="scope")
def cache(cfg: dict[str, str] = Depends(app_config)) -> str:
    return f"Cache({cfg['cache_url']})"


@fixture
def user_service(
    db_svc: str = Depends(database),
    cache_svc: str = Depends(cache),
) -> str:
    return f"UserService({db_svc}, {cache_svc})"


with describe("composability"):

    @test(name="test receives fully resolved dependency chain")
    def test_composed_service(
        svc: str = Depends(user_service),
        cfg: dict[str, str] = Depends(app_config),
        db_svc: str = Depends(database),
        cache_svc: str = Depends(cache),
    ) -> None:
        expect(cfg, "app config resolved").to_equal(
            {"db_url": "sqlite:///:memory:", "cache_url": "redis://localhost"}
        )
        expect(db_svc, "database built from config").to_equal(
            "Database(sqlite:///:memory:)"
        )
        expect(cache_svc, "cache built from config").to_equal(
            "Cache(redis://localhost)"
        )
        expect(svc, "user service composed from db and cache").to_equal(
            "UserService(Database(sqlite:///:memory:), Cache(redis://localhost))"
        )

    @test(name="per-test fixture is fresh, per-scope is reused")
    def test_fresh_test_reused_scope(
        svc: str = Depends(user_service),
    ) -> None:
        expect(svc, "fresh per-test svc still sees cached per-scope deps").to_equal(
            "UserService(Database(sqlite:///:memory:), Cache(redis://localhost))"
        )


@fixture(per="scope")
def base_url() -> str:
    return "http://localhost:8000"


with describe("composability > nested describe"):

    @fixture
    def auth_header(url: str = Depends(base_url)) -> dict[str, str]:
        return {"Authorization": f"Bearer token-for-{url}"}

    @test(name="describe-scoped fixture depends on module-scoped per='scope'")
    def test_nested_depends(
        header: dict[str, str] = Depends(auth_header),
    ) -> None:
        expect(header, "header has bearer token built from base url").to_equal(
            {"Authorization": "Bearer token-for-http://localhost:8000"}
        )
