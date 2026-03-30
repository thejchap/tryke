"""Tests for the hooks module: decorators, Depends(), and resolver."""

from __future__ import annotations

from typing import TYPE_CHECKING, assert_type

import tryke
from tryke import describe, expect, test
from tryke.hooks import (
    CyclicDependencyError,
    DependencyResolver,
    Depends,
    HookExecutor,
    _Depends,
    after_all,
    after_each,
    before_all,
    before_each,
    wrap_all,
    wrap_each,
)

if TYPE_CHECKING:
    from collections.abc import Generator

with describe("hook decorators"):

    @test(name="before_each stamps attribute on function")
    def test_before_each_stamps() -> None:
        @before_each
        def setup() -> int:
            return 42

        expect(hasattr(setup, "__tryke_before_each__")).to_be_truthy()
        expect(setup()).to_equal(42)

    @test(name="before_all stamps attribute on function")
    def test_before_all_stamps() -> None:
        @before_all
        def setup() -> str:
            return "db"

        expect(hasattr(setup, "__tryke_before_all__")).to_be_truthy()
        expect(setup()).to_equal("db")

    @test(name="after_each stamps attribute on function")
    def test_after_each_stamps() -> None:
        @after_each
        def cleanup() -> None:
            pass

        expect(hasattr(cleanup, "__tryke_after_each__")).to_be_truthy()

    @test(name="after_all stamps attribute on function")
    def test_after_all_stamps() -> None:
        @after_all
        def cleanup() -> None:
            pass

        expect(hasattr(cleanup, "__tryke_after_all__")).to_be_truthy()

    @test(name="wrap_each stamps attribute on function")
    def test_wrap_each_stamps() -> None:
        @wrap_each
        def wrapper() -> Generator[int, None, None]:
            yield 42

        expect(hasattr(wrapper, "__tryke_wrap_each__")).to_be_truthy()

    @test(name="wrap_all stamps attribute on function")
    def test_wrap_all_stamps() -> None:
        @wrap_all
        def wrapper() -> Generator[int, None, None]:
            yield 42

        expect(hasattr(wrapper, "__tryke_wrap_all__")).to_be_truthy()

    @test(name="bare decorator form works")
    def test_bare_form() -> None:
        @before_each
        def setup() -> int:
            return 1

        expect(hasattr(setup, "__tryke_before_each__")).to_be_truthy()
        expect(setup()).to_equal(1)

    @test(name="call decorator form works")
    def test_call_form() -> None:
        @before_each()
        def setup() -> int:
            return 1

        expect(hasattr(setup, "__tryke_before_each__")).to_be_truthy()
        expect(setup()).to_equal(1)

    @test(name="decorated function is unchanged")
    def test_function_unchanged() -> None:
        def original() -> int:
            return 99

        decorated = before_each(original)
        expect(decorated).to_be(original)
        expect(decorated()).to_equal(99)


with describe("Depends"):

    @test(name="Depends returns _Depends instance")
    def test_depends_returns_sentinel() -> None:
        def my_hook() -> int:
            return 42

        dep = Depends(my_hook)
        expect(isinstance(dep, _Depends)).to_be_truthy()
        expect(dep.dependency).to_be(my_hook)

    @test(name="Depends stores the dependency callable")
    def test_depends_stores_callable() -> None:
        def hook_a() -> str:
            return "a"

        def hook_b() -> str:
            return "b"

        dep_a = Depends(hook_a)
        dep_b = Depends(hook_b)
        expect(dep_a.dependency).to_be(hook_a)
        expect(dep_b.dependency).to_be(hook_b)

    @test(name="_Depends is frozen")
    def test_depends_frozen() -> None:
        def my_hook() -> int:
            return 1

        dep = Depends(my_hook)
        expect(lambda: setattr(dep, "dependency", None)).to_raise(AttributeError)


with describe("public exports"):

    @test(name="hooks are exported from tryke package")
    def test_exports() -> None:
        expect(hasattr(tryke, "before_each")).to_be_truthy()
        expect(hasattr(tryke, "before_all")).to_be_truthy()
        expect(hasattr(tryke, "after_each")).to_be_truthy()
        expect(hasattr(tryke, "after_all")).to_be_truthy()
        expect(hasattr(tryke, "wrap_each")).to_be_truthy()
        expect(hasattr(tryke, "wrap_all")).to_be_truthy()
        expect(hasattr(tryke, "Depends")).to_be_truthy()


with describe("DependencyResolver"):

    @test(name="resolves a simple Depends chain")
    def test_resolve_simple() -> None:
        @before_all
        def db() -> str:
            return "conn"

        @before_all
        def table(conn: str = Depends(db)) -> str:
            return f"{conn}/table"

        resolver = DependencyResolver()
        result = resolver.resolve(table)
        expect(result).to_equal({"conn": "conn"})

    @test(name="caches resolved values")
    def test_caching() -> None:
        call_count = 0

        @before_each
        def counter() -> int:
            nonlocal call_count
            call_count += 1
            return call_count

        @before_each
        def user_a(c: int = Depends(counter)) -> str:
            return f"a:{c}"

        @before_each
        def user_b(c: int = Depends(counter)) -> str:
            return f"b:{c}"

        resolver = DependencyResolver()
        a = resolver.resolve(user_a)
        b = resolver.resolve(user_b)
        # Both should get the same cached value
        expect(a["c"]).to_equal(1)
        expect(b["c"]).to_equal(1)
        expect(call_count).to_equal(1)

    @test(name="detects dependency cycles")
    def test_cycle_detection() -> None:
        @before_each
        def hook_a(_b: str = Depends(lambda: None)) -> str:  # Placeholder
            return "a"

        @before_each
        def hook_b(_a: str = Depends(hook_a)) -> str:
            return "b"

        # Manually wire the cycle: hook_a depends on hook_b
        hook_a.__defaults__ = (Depends(hook_b),)

        resolver = DependencyResolver()
        expect(lambda: resolver.resolve(hook_a)).to_raise(CyclicDependencyError)

    @test(name="resolves generator hooks via next()")
    def test_generator_resolution() -> None:
        teardown_ran = False

        @wrap_each
        def with_resource() -> Generator[str, None, None]:
            nonlocal teardown_ran
            yield "resource"
            teardown_ran = True

        resolver = DependencyResolver()
        value = resolver.resolve_hook(with_resource)
        expect(value).to_equal("resource")
        expect(teardown_ran).to_be_falsy()

        resolver.teardown_generators()
        expect(teardown_ran).to_be_truthy()

    @test(name="clear_each_cache resets per-test state")
    def test_clear_each_cache() -> None:
        call_count = 0

        @before_each
        def counter() -> int:
            nonlocal call_count
            call_count += 1
            return call_count

        resolver = DependencyResolver()
        v1 = resolver.resolve_hook(counter)
        expect(v1).to_equal(1)

        resolver.clear_each_cache()
        v2 = resolver.resolve_hook(counter)
        expect(v2).to_equal(2)


with describe("HookExecutor"):

    @test(name="runs before_each hooks before test")
    def test_before_each_runs() -> None:
        log: list[str] = []

        @before_each
        def setup() -> None:
            log.append("setup")

        def my_test() -> None:
            log.append("test")

        executor = HookExecutor()
        executor.register_hook(setup, groups=[])
        executor.run_test(my_test, groups=[])
        expect(log).to_equal(["setup", "test"])

    @test(name="runs after_each hooks after test")
    def test_after_each_runs() -> None:
        log: list[str] = []

        @after_each
        def cleanup() -> None:
            log.append("cleanup")

        def my_test() -> None:
            log.append("test")

        executor = HookExecutor()
        executor.register_hook(cleanup, groups=[])
        executor.run_test(my_test, groups=[])
        expect(log).to_equal(["test", "cleanup"])

    @test(name="wrap_each wraps around test")
    def test_wrap_each_wraps() -> None:
        log: list[str] = []

        @wrap_each
        def wrapper() -> Generator[None, None, None]:
            log.append("setup")
            yield
            log.append("teardown")

        def my_test() -> None:
            log.append("test")

        executor = HookExecutor()
        executor.register_hook(wrapper, groups=[])
        executor.run_test(my_test, groups=[])
        expect(log).to_equal(["setup", "test", "teardown"])

    @test(name="outer scope hooks wrap inner scope hooks")
    def test_scope_nesting() -> None:
        log: list[str] = []

        @before_each
        def outer_setup() -> None:
            log.append("outer")

        @before_each
        def inner_setup() -> None:
            log.append("inner")

        def my_test() -> None:
            log.append("test")

        executor = HookExecutor()
        executor.register_hook(outer_setup, groups=[])
        executor.register_hook(inner_setup, groups=["users"])
        executor.run_test(my_test, groups=["users"])
        expect(log).to_equal(["outer", "inner", "test"])

    @test(name="after hooks run in reverse definition order")
    def test_after_reverse_order() -> None:
        log: list[str] = []

        @after_each
        def first_cleanup() -> None:
            log.append("first")

        @after_each
        def second_cleanup() -> None:
            log.append("second")

        def my_test() -> None:
            log.append("test")

        executor = HookExecutor()
        executor.register_hook(first_cleanup, groups=[], line_number=1)
        executor.register_hook(second_cleanup, groups=[], line_number=2)
        executor.run_test(my_test, groups=[])
        # After hooks run bottom-to-top (stack unwinding)
        expect(log).to_equal(["test", "second", "first"])

    @test(name="test can receive values via Depends")
    def test_depends_in_test() -> None:
        @before_each
        def db() -> str:
            return "conn"

        received = {}

        def my_test(conn: str = Depends(db)) -> None:
            received["conn"] = conn

        executor = HookExecutor()
        executor.register_hook(db, groups=[])
        executor.run_test(my_test, groups=[])
        expect(received["conn"]).to_equal("conn")


with describe("Depends typing"):

    @test(name="assert_type validates Depends return type for plain function")
    def test_depends_type_plain() -> None:
        @before_all
        def db() -> str:
            return "conn"

        # At type-check time: Depends(db) should be str
        val = Depends(db)
        assert_type(val, str)

    @test(name="assert_type validates Depends return type for generator")
    def test_depends_type_generator() -> None:
        @wrap_each
        def resource() -> Generator[int, None, None]:
            yield 42

        # At type-check time: Depends(resource) should be int (unwrapped from Generator)
        val = Depends(resource)
        assert_type(val, int)
