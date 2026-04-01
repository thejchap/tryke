"""Hook decorators and Depends() for test setup/teardown.

Provides six lifecycle hooks that can be scoped to describe blocks
or module level, and a FastAPI-style ``Depends()`` for typed,
explicit dependency injection between hooks and tests.
"""

from __future__ import annotations

import asyncio
import contextlib
import inspect
from dataclasses import dataclass
from typing import TYPE_CHECKING, overload

if TYPE_CHECKING:
    from collections.abc import AsyncGenerator, Callable, Generator
    from typing import Any, TypeVar

    T = TypeVar("T")

# ---------------------------------------------------------------------------
# Depends
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class _Depends:
    """Sentinel returned by :func:`Depends` at runtime.

    The worker inspects function signatures for ``_Depends`` defaults
    and resolves them before calling the function.
    """

    dependency: Callable[..., Any]


if TYPE_CHECKING:

    @overload
    def Depends(dep: Callable[..., Generator[T, None, None]], /) -> T: ...  # noqa: UP047
    @overload
    def Depends(dep: Callable[..., AsyncGenerator[T, None]], /) -> T: ...  # noqa: UP047
    @overload
    def Depends(dep: Callable[..., T], /) -> T: ...  # noqa: UP047


def Depends(dep: Callable[..., Any], /) -> Any:  # noqa: N802 - matches FastAPI convention
    """Declare a dependency on another hook or fixture.

    Used in function signatures to request a resolved value::

        @before_all
        def db() -> Connection:
            return create_connection()

        @test
        def my_test(conn: Connection = Depends(db)):
            ...

    The type checker sees ``Depends(db)`` as returning ``Connection``.
    At runtime it returns a :class:`_Depends` sentinel that the worker
    resolves before calling the function.
    """
    return _Depends(dep)


# ---------------------------------------------------------------------------
# Hook decorators
# ---------------------------------------------------------------------------

type _Fn = Callable[..., Any]


def _make_hook_decorator(attr: str) -> Callable[..., Any]:
    """Build a decorator that stamps *attr* on the decorated function.

    Supports both bare (``@before_each``) and call (``@before_each()``)
    forms.
    """

    def decorator(fn: _Fn | None = None, /) -> _Fn | Callable[[_Fn], _Fn]:
        if fn is not None:
            setattr(fn, attr, True)
            return fn

        # Call form: @hook()
        def inner(f: _Fn) -> _Fn:
            setattr(f, attr, True)
            return f

        return inner

    return decorator


before_each = _make_hook_decorator("__tryke_before_each__")
before_all = _make_hook_decorator("__tryke_before_all__")
after_each = _make_hook_decorator("__tryke_after_each__")
after_all = _make_hook_decorator("__tryke_after_all__")
wrap_each = _make_hook_decorator("__tryke_wrap_each__")
wrap_all = _make_hook_decorator("__tryke_wrap_all__")

# Dunder → hook category mapping.
_HOOK_ATTRS = {
    "__tryke_before_each__": "before_each",
    "__tryke_before_all__": "before_all",
    "__tryke_after_each__": "after_each",
    "__tryke_after_all__": "after_all",
    "__tryke_wrap_each__": "wrap_each",
    "__tryke_wrap_all__": "wrap_all",
}

_EACH_CATEGORIES = {"before_each", "after_each", "wrap_each"}


def _hook_category(fn: Callable[..., Any]) -> str | None:
    """Return the hook category for a stamped function, or None."""
    for attr, category in _HOOK_ATTRS.items():
        if hasattr(fn, attr):
            return category
    return None


# ---------------------------------------------------------------------------
# Exceptions
# ---------------------------------------------------------------------------


class CyclicDependencyError(Exception):
    """Raised when Depends() forms a cycle."""


# ---------------------------------------------------------------------------
# DependencyResolver
# ---------------------------------------------------------------------------


class DependencyResolver:
    """Resolve ``Depends()`` in function signatures, cache results by scope."""

    def __init__(self) -> None:
        self._cache: dict[Callable[..., Any], Any] = {}
        self._active_generators: list[
            tuple[Callable[..., Any], Generator[Any, None, None], bool]
        ] = []  # (fn, gen, is_all_scope)
        self._active_async_generators: list[
            tuple[
                Callable[..., Any],
                AsyncGenerator[Any, None],
                asyncio.AbstractEventLoop,
                bool,
            ]
        ] = []  # (fn, agen, loop, is_all_scope)
        self._resolving: set[int] = set()  # ids of functions currently being resolved

    def resolve(self, fn: Callable[..., Any]) -> dict[str, Any]:
        """Inspect *fn*'s signature and resolve all ``Depends()`` defaults.

        Returns a dict of ``{param_name: resolved_value}`` suitable for
        passing as keyword arguments.
        """
        kwargs: dict[str, Any] = {}
        sig = inspect.signature(fn)
        for name, param in sig.parameters.items():
            if isinstance(param.default, _Depends):
                dep_fn = param.default.dependency
                kwargs[name] = self.resolve_hook(dep_fn)
        return kwargs

    def resolve_hook(self, fn: Callable[..., Any], *, all_scope: bool = False) -> Any:  # noqa: ANN401
        """Resolve a single hook function, returning its cached value.

        When *all_scope* is True, generator lifecycles are preserved until
        :meth:`teardown_all_generators` rather than :meth:`teardown_generators`.
        """
        if fn in self._cache:
            return self._cache[fn]

        fn_id = id(fn)
        if fn_id in self._resolving:
            msg = f"Cyclic dependency detected involving {fn.__name__}"
            raise CyclicDependencyError(msg)

        self._resolving.add(fn_id)
        try:
            # Recursively resolve this hook's own dependencies
            kwargs = self.resolve(fn)

            if inspect.isgeneratorfunction(fn):
                gen = fn(**kwargs)
                value = next(gen)
                self._active_generators.append((fn, gen, all_scope))
            elif inspect.isasyncgenfunction(fn):
                agen = fn(**kwargs)
                loop = asyncio.new_event_loop()
                value = loop.run_until_complete(agen.__anext__())
                self._active_async_generators.append((fn, agen, loop, all_scope))
            elif inspect.iscoroutinefunction(fn):
                value = asyncio.run(fn(**kwargs))
            else:
                value = fn(**kwargs)

            self._cache[fn] = value
            return value
        finally:
            self._resolving.discard(fn_id)

    def clear_each_cache(self) -> None:
        """Clear cached values for per-test hooks (``_each`` category)."""
        to_remove = [fn for fn in self._cache if _hook_category(fn) in _EACH_CATEGORIES]
        for fn in to_remove:
            del self._cache[fn]

    def _teardown_sync_generators(
        self,
        gens: list[tuple[Callable[..., Any], Generator[Any, None, None], bool]],
    ) -> None:
        while gens:
            fn, gen, _all = gens.pop()
            try:
                try:
                    next(gen)
                except StopIteration:
                    continue
                else:
                    msg = (
                        f"Generator hook {fn.__name__} yielded more than once; "
                        "only single-yield hooks are supported."
                    )
                    raise RuntimeError(msg)
            finally:
                with contextlib.suppress(Exception):
                    gen.close()

    def _teardown_async_generators(
        self,
        gens: list[
            tuple[
                Callable[..., Any],
                AsyncGenerator[Any, None],
                asyncio.AbstractEventLoop,
                bool,
            ]
        ],
    ) -> None:
        while gens:
            fn, agen, loop, _all = gens.pop()
            try:
                try:
                    loop.run_until_complete(agen.__anext__())
                except StopAsyncIteration:
                    continue
                else:
                    msg = (
                        f"Async generator hook {fn.__name__} yielded more than once; "
                        "only single-yield hooks are supported."
                    )
                    raise RuntimeError(msg)
            finally:
                with contextlib.suppress(Exception):
                    loop.run_until_complete(agen.aclose())
                with contextlib.suppress(Exception):
                    loop.close()

    def teardown_generators(self) -> None:
        """Run the post-yield portion of per-test (``_each``) generators (LIFO)."""
        # Partition: keep _all generators for later, tear down _each now.
        keep = [entry for entry in self._active_generators if entry[2]]
        tear = [entry for entry in self._active_generators if not entry[2]]
        self._active_generators = keep
        self._teardown_sync_generators(tear)

        keep_async = [entry for entry in self._active_async_generators if entry[3]]
        tear_async = [entry for entry in self._active_async_generators if not entry[3]]
        self._active_async_generators = keep_async
        self._teardown_async_generators(tear_async)

    def teardown_all_generators(self) -> None:
        """Run the post-yield portion of scope-level (``_all``) generators (LIFO)."""
        self._teardown_sync_generators(self._active_generators)
        self._active_generators = []
        self._teardown_async_generators(self._active_async_generators)
        self._active_async_generators = []

    def clear_all(self) -> None:
        """Reset all state."""
        self.teardown_generators()
        self.teardown_all_generators()
        self._cache.clear()
        self._resolving.clear()


# ---------------------------------------------------------------------------
# HookExecutor
# ---------------------------------------------------------------------------


@dataclass
class _RegisteredHook:
    """A hook function with its metadata."""

    fn: Callable[..., Any]
    category: str
    groups: list[str]
    line_number: int = 0


class HookExecutor:
    """Orchestrate hook execution around tests.

    Hooks are registered with their scope (groups) and executed in the
    correct order: outer-to-inner for setup, inner-to-outer for teardown.
    """

    def __init__(self) -> None:
        self._hooks: list[_RegisteredHook] = []
        self._resolver = DependencyResolver()
        self._all_initialized: set[tuple[str, ...]] = set()
        # Tracks every scope visited by run_test, so finalize only fires
        # after_all for scopes that actually had tests run in them.
        self._visited_scopes: set[tuple[str, ...]] = set()

    def register_hook(
        self,
        fn: Callable[..., Any],
        *,
        groups: list[str],
        line_number: int = 0,
    ) -> None:
        """Register a hook function with its scope."""
        category = _hook_category(fn)
        if category is None:
            msg = f"{fn.__name__} is not a decorated hook"
            raise ValueError(msg)
        self._hooks.append(
            _RegisteredHook(
                fn=fn, category=category, groups=groups, line_number=line_number
            )
        )

    def run_test(
        self,
        test_fn: Callable[..., Any],
        *,
        groups: list[str],
    ) -> None:
        """Execute hooks in correct order around *test_fn*."""
        # Build the scope chain: [], ["a"], ["a", "b"], ...
        scopes: list[tuple[str, ...]] = [
            (),
            *[tuple(groups[: i + 1]) for i in range(len(groups))],
        ]

        # Record all scopes visited by this test run.
        self._visited_scopes.update(scopes)

        # Collect hooks per scope
        setup_sequence: list[_RegisteredHook] = []
        teardown_sequence: list[_RegisteredHook] = []

        for scope in scopes:
            scope_hooks = [h for h in self._hooks if tuple(h.groups) == scope]

            # Before_all: run once per scope
            for h in sorted(
                (h for h in scope_hooks if h.category == "before_all"),
                key=lambda h: h.line_number,
            ):
                if (h.category, h.fn, *h.groups) not in self._all_initialized:  # type: ignore[arg-type]
                    self._resolver.resolve_hook(h.fn, all_scope=True)
                    self._all_initialized.add((h.category, h.fn, *h.groups))  # type: ignore[arg-type]

            # Wrap_all: resolve on first test in scope (like before_all).
            # Generator setup runs now; teardown deferred to finalize().
            for h in sorted(
                (h for h in scope_hooks if h.category == "wrap_all"),
                key=lambda h: h.line_number,
            ):
                if (h.category, h.fn, *h.groups) not in self._all_initialized:  # type: ignore[arg-type]
                    self._resolver.resolve_hook(h.fn, all_scope=True)
                    self._all_initialized.add((h.category, h.fn, *h.groups))  # type: ignore[arg-type]

            # Before_each: setup order (definition order)
            setup_sequence.extend(
                sorted(
                    (h for h in scope_hooks if h.category == "before_each"),
                    key=lambda h: h.line_number,
                )
            )

            # Wrap_each: setup half
            setup_sequence.extend(
                sorted(
                    (h for h in scope_hooks if h.category == "wrap_each"),
                    key=lambda h: h.line_number,
                )
            )

            # After_each: collect in definition order; final reversal handles
            # both inner-to-outer and within-scope reverse ordering.
            teardown_sequence.extend(
                sorted(
                    (h for h in scope_hooks if h.category == "after_each"),
                    key=lambda h: h.line_number,
                )
            )

        # Run setup
        for h in setup_sequence:
            self._resolver.resolve_hook(h.fn)

        # Run test with Depends injection, ensuring teardown always runs.
        try:
            test_kwargs = self._resolver.resolve(test_fn)
            if inspect.iscoroutinefunction(test_fn):
                asyncio.run(test_fn(**test_kwargs))
            else:
                test_fn(**test_kwargs)
        finally:
            # Teardown wrap_each generators first (wrap semantics: closest to test)
            self._resolver.teardown_generators()

            # Run after_each teardown (inner-to-outer, reverse order)
            teardown_sequence.reverse()
            for h in teardown_sequence:
                kwargs = self._resolver.resolve(h.fn)
                if inspect.iscoroutinefunction(h.fn):
                    asyncio.run(h.fn(**kwargs))
                else:
                    h.fn(**kwargs)

            # Clear per-test caches
            self._resolver.clear_each_cache()

    def finalize(self) -> None:
        """Run scope-level teardown: after_all hooks and wrap_all generators.

        The worker calls this after all tests in a module have completed.
        Only processes scopes that were actually entered during test execution.
        """
        # Process scopes inner-to-outer (longest first for reverse ordering)
        sorted_scopes = sorted(self._visited_scopes, key=len, reverse=True)

        # Collect after_all hooks across initialized scopes (inner-to-outer)
        after_all_hooks: list[_RegisteredHook] = []
        for scope in sorted_scopes:
            scope_hooks = [h for h in self._hooks if tuple(h.groups) == scope]
            # Within a scope, collect in definition order; the reversal
            # at the end handles within-scope LIFO.
            after_all_hooks.extend(
                sorted(
                    (h for h in scope_hooks if h.category == "after_all"),
                    key=lambda h: h.line_number,
                )
            )

        # Run after_all in reverse order (inner-to-outer, LIFO within scope)
        after_all_hooks.reverse()
        for h in after_all_hooks:
            kwargs = self._resolver.resolve(h.fn)
            if inspect.iscoroutinefunction(h.fn):
                asyncio.run(h.fn(**kwargs))
            else:
                h.fn(**kwargs)

        # Teardown wrap_all generators (stored by the resolver during
        # resolve_hook when wrap_all was first initialized).
        self._resolver.teardown_all_generators()
        self._resolver.clear_all()
