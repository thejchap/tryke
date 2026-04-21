"""Fixture decorator and Depends() for test setup/teardown.

Provides a single `@fixture` decorator with `per="test"` (default) or
`per="scope"` granularity. A fixture runs setup before each covered test
(or once per lexical scope) and — if it ``yield``s — teardown after.

``Depends()`` wires typed dependency injection between fixtures and
tests, FastAPI-style.
"""

from __future__ import annotations

import asyncio
import contextlib
import inspect
from dataclasses import dataclass
from typing import TYPE_CHECKING, Literal, NamedTuple, overload

if TYPE_CHECKING:
    from collections.abc import AsyncGenerator, Awaitable, Callable, Generator
    from typing import Any, Protocol

    from tryke.expect import CaseArgs

    class _FixtureFn(Protocol):
        """A named callable — i.e. any real Python function.

        Used as the bound for generic type variables in the ``@fixture``
        decorator so that the inferred type variable carries the full
        concrete signature of the decorated function (including its
        return type), letting ``Depends()`` resolve downstream without
        losing precision.
        """

        __name__: str

        def __call__(self, /, *args: Any, **kwargs: Any) -> Any: ...  # noqa: ANN401


# Per-test vs per-(lexical-)scope granularity.
FixturePer = Literal["test", "scope"]

# Attribute name stamped onto decorated functions. The value is one of
# ``FixturePer``.
_FIXTURE_ATTR = "__tryke_fixture__"


# ---------------------------------------------------------------------------
# Depends
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class _Depends:
    """Sentinel returned by :func:`Depends` at runtime.

    The executor inspects function signatures for ``_Depends`` defaults
    and resolves them before calling the function.
    """

    dependency: Callable[..., Any]


if TYPE_CHECKING:

    @overload
    def Depends[T](dep: Callable[..., Generator[T, None, None]], /) -> T: ...
    @overload
    def Depends[T](dep: Callable[..., AsyncGenerator[T, None]], /) -> T: ...
    @overload
    def Depends[T](dep: Callable[..., Awaitable[T]], /) -> T: ...
    @overload
    def Depends[T](dep: Callable[..., T], /) -> T: ...


def Depends(dep: Callable[..., Any], /) -> Any:  # noqa: N802 - matches FastAPI convention
    """Declare a dependency on another fixture.

    Used in function signatures to request a resolved value::

        @fixture(per="scope")
        def db() -> Connection:
            return create_connection()

        @test
        def my_test(conn: Connection = Depends(db)):
            ...

    Type checkers see ``Depends(db)`` as returning ``Connection``. At
    runtime it returns a :class:`_Depends` sentinel that the executor
    resolves before calling the function.
    """
    return _Depends(dep)


# ---------------------------------------------------------------------------
# @fixture decorator
# ---------------------------------------------------------------------------


@overload
def fixture[F: _FixtureFn](fn: F, /) -> F: ...
@overload
def fixture[F: _FixtureFn](
    *,
    per: FixturePer = "test",
) -> Callable[[F], F]: ...
def fixture(
    fn: _FixtureFn | None = None,
    /,
    *,
    per: FixturePer = "test",
) -> _FixtureFn | Callable[[_FixtureFn], _FixtureFn]:
    """Mark a function as a tryke fixture.

    A fixture runs automatically around every test in its lexical scope
    (module-level fixtures cover all tests in the file; fixtures defined
    inside ``with describe(...)`` cover tests in that describe block).

    Use ``yield`` to split setup and teardown::

        @fixture
        def db():
            conn = connect()
            yield conn          # value visible to tests via Depends()
            conn.close()        # teardown runs after each test

    Use ``per="scope"`` to cache the value across every test in the
    fixture's lexical scope — the function runs once, and teardown runs
    after the last test in that scope::

        @fixture(per="scope")
        def app():
            return TestApp()    # plain return: no teardown

    Tests (and other fixtures) consume values with :func:`Depends`.
    """
    if fn is not None:
        # Bare-decorator form: @fixture (no parentheses). `per` defaults
        # to "test" because that is the only form this branch handles.
        setattr(fn, _FIXTURE_ATTR, "test")
        return fn

    def inner(f: _FixtureFn) -> _FixtureFn:
        setattr(f, _FIXTURE_ATTR, per)
        return f

    return inner


def _fixture_per(fn: object) -> FixturePer | None:
    """Return the fixture granularity for a decorated function, or None."""
    value = getattr(fn, _FIXTURE_ATTR, None)
    if value in {"test", "scope"}:
        return value
    return None


# ---------------------------------------------------------------------------
# Exceptions
# ---------------------------------------------------------------------------


class CyclicDependencyError(Exception):
    """Raised when Depends() forms a cycle."""


class HookError(Exception):
    """Raised when fixture resolution or execution violates the lifecycle."""


# ---------------------------------------------------------------------------
# DependencyResolver
# ---------------------------------------------------------------------------


class _SyncGenEntry(NamedTuple):
    fn: _FixtureFn
    gen: Generator[Any, None, None]
    is_scope: bool


class _AsyncGenEntry(NamedTuple):
    fn: _FixtureFn
    agen: AsyncGenerator[Any, None]
    loop: asyncio.AbstractEventLoop
    is_scope: bool
    owns_loop: bool


class DependencyResolver:
    """Resolve ``Depends()`` in function signatures and cache results.

    Values are cached by function identity. ``per="test"`` values are
    cleared between tests; ``per="scope"`` values persist until
    :meth:`clear_all`.
    """

    def __init__(self) -> None:
        self._cache: dict[_FixtureFn, Any] = {}
        self._scope_fixtures: set[_FixtureFn] = set()
        self._active_generators: list[_SyncGenEntry] = []
        self._active_async_generators: list[_AsyncGenEntry] = []
        self._resolving: set[int] = set()
        # Shared loop for async fixtures + async test bodies. Lazily
        # created on first async need so pure-sync modules pay nothing.
        # Sharing one loop across the resolver's lifetime is what makes
        # `async with`-style fixtures compose with async tests: without
        # it, a fixture created on loop A returns loop-bound objects that
        # the test body — running on a freshly-minted loop B — cannot
        self._shared_loop: asyncio.AbstractEventLoop | None = None

    @property
    def shared_loop(self) -> asyncio.AbstractEventLoop:
        """Return the resolver's shared event loop, creating it on demand.

        All async work (fixture setup, async test bodies, async fixture
        teardown) that goes through this resolver runs on the returned
        loop. The loop lives until :meth:`close_shared_loop` is called,
        typically from ``HookExecutor.finalize``.
        """
        if self._shared_loop is None or self._shared_loop.is_closed():
            self._shared_loop = asyncio.new_event_loop()
            asyncio.set_event_loop(self._shared_loop)
        return self._shared_loop

    def close_shared_loop(self) -> None:
        """Close the shared event loop if one was lazily created."""
        loop = self._shared_loop
        self._shared_loop = None
        if loop is None:
            return
        with contextlib.suppress(Exception):
            asyncio.set_event_loop(None)
        with contextlib.suppress(Exception):
            loop.close()

    def resolve(self, fn: _FixtureFn) -> dict[str, Any]:
        """Inspect *fn*'s signature and resolve all ``Depends()`` defaults.

        Returns a dict of ``{param_name: resolved_value}`` suitable for
        passing as keyword arguments.
        """
        kwargs: dict[str, Any] = {}
        sig = inspect.signature(fn)
        for name, param in sig.parameters.items():
            if isinstance(param.default, _Depends):
                kwargs[name] = self.resolve_hook(param.default.dependency)
        return kwargs

    def resolve_hook(self, fn: _FixtureFn) -> Any:  # noqa: ANN401
        """Resolve a single fixture, returning its cached value.

        The fixture's ``per`` attribute determines whether it is tracked
        as per-test or per-scope. Plain callables (no ``@fixture``
        decorator) default to per-test.
        """
        if fn in self._cache:
            return self._cache[fn]

        fn_id = id(fn)
        if fn_id in self._resolving:
            msg = f"Cyclic dependency detected involving {fn.__name__}"
            raise CyclicDependencyError(msg)

        per = _fixture_per(fn) or "test"
        is_scope = per == "scope"

        self._resolving.add(fn_id)
        try:
            kwargs = self.resolve(fn)

            if inspect.isgeneratorfunction(fn):
                gen = fn(**kwargs)
                value = next(gen)
                self._active_generators.append(_SyncGenEntry(fn, gen, is_scope))
            elif inspect.isasyncgenfunction(fn):
                agen = fn(**kwargs)
                loop = self.shared_loop
                value = loop.run_until_complete(agen.__anext__())
                self._active_async_generators.append(
                    _AsyncGenEntry(fn, agen, loop, is_scope, owns_loop=False)
                )
            elif inspect.iscoroutinefunction(fn):
                value = self.shared_loop.run_until_complete(fn(**kwargs))
            else:
                value = fn(**kwargs)

            self._cache[fn] = value
            if is_scope:
                self._scope_fixtures.add(fn)
            return value
        finally:
            self._resolving.discard(fn_id)

    def clear_test_cache(self) -> None:
        """Drop per-test cached values so the next test resolves fresh."""
        to_remove = [fn for fn in self._cache if fn not in self._scope_fixtures]
        for fn in to_remove:
            del self._cache[fn]

    def _teardown_sync_generators(self, gens: list[_SyncGenEntry]) -> None:
        while gens:
            entry = gens.pop()
            try:
                try:
                    next(entry.gen)
                except StopIteration:
                    pass
                else:
                    msg = (
                        f"Fixture {entry.fn.__name__} yielded more than once; "
                        "only single-yield fixtures are supported."
                    )
                    raise RuntimeError(msg)
            finally:
                with contextlib.suppress(Exception):
                    entry.gen.close()
                self._cache.pop(entry.fn, None)
                if entry.is_scope:
                    self._scope_fixtures.discard(entry.fn)

    def _teardown_async_generators(self, gens: list[_AsyncGenEntry]) -> None:
        while gens:
            entry = gens.pop()
            try:
                try:
                    entry.loop.run_until_complete(entry.agen.__anext__())
                except StopAsyncIteration:
                    pass
                else:
                    msg = (
                        f"Async fixture {entry.fn.__name__} yielded more than once; "
                        "only single-yield fixtures are supported."
                    )
                    raise RuntimeError(msg)
            finally:
                with contextlib.suppress(Exception):
                    entry.loop.run_until_complete(entry.agen.aclose())
                if entry.owns_loop:
                    with contextlib.suppress(Exception):
                        entry.loop.close()
                self._cache.pop(entry.fn, None)
                if entry.is_scope:
                    self._scope_fixtures.discard(entry.fn)

    def teardown_test_generators(self) -> None:
        """Run the post-yield portion of per-test fixtures (LIFO)."""
        keep = [e for e in self._active_generators if e.is_scope]
        tear = [e for e in self._active_generators if not e.is_scope]
        self._active_generators = keep
        self._teardown_sync_generators(tear)

        keep_async = [e for e in self._active_async_generators if e.is_scope]
        tear_async = [e for e in self._active_async_generators if not e.is_scope]
        self._active_async_generators = keep_async
        self._teardown_async_generators(tear_async)

    def teardown_scope_generators(self) -> None:
        """Run the post-yield portion of per-scope fixtures (LIFO)."""
        self._teardown_sync_generators(self._active_generators)
        self._active_generators = []
        self._teardown_async_generators(self._active_async_generators)
        self._active_async_generators = []

    def clear_all(self) -> None:
        """Reset all state."""
        self.teardown_test_generators()
        self.teardown_scope_generators()
        self._cache.clear()
        self._scope_fixtures.clear()
        self._resolving.clear()


# ---------------------------------------------------------------------------
# HookExecutor
# ---------------------------------------------------------------------------


@dataclass
class _RegisteredFixture:
    """A fixture function with its metadata."""

    fn: _FixtureFn
    per: FixturePer
    groups: list[str]
    line_number: int = 0


class _ScopeKey(NamedTuple):
    """Identity key for tracking which (fn, scope) combos have initialized."""

    fn_id: int
    groups: tuple[str, ...]


class HookExecutor:
    """Orchestrate fixture execution around tests.

    Fixtures are registered with their scope (groups) and executed in the
    correct order: outer-to-inner for setup, inner-to-outer for teardown.
    Within a single scope, fixtures run in definition order for setup and
    in reverse for teardown (LIFO, matching how ``contextlib.ExitStack``
    and pytest's fixture teardown behave).
    """

    def __init__(self) -> None:
        self._fixtures: list[_RegisteredFixture] = []
        self._resolver = DependencyResolver()
        self._scope_initialized: set[_ScopeKey] = set()
        # Every scope visited by run_test — finalize() only tears down
        # per-scope fixtures for scopes that actually had tests run.
        self._visited_scopes: set[tuple[str, ...]] = set()

    def register_fixture(
        self,
        fn: _FixtureFn,
        *,
        groups: list[str],
        line_number: int = 0,
    ) -> None:
        """Register a fixture function with its lexical scope."""
        per = _fixture_per(fn)
        if per is None:
            msg = f"{fn.__name__} is not decorated with @fixture"
            raise ValueError(msg)
        self._fixtures.append(
            _RegisteredFixture(fn=fn, per=per, groups=groups, line_number=line_number)
        )

    def run_test(
        self,
        test_fn: _FixtureFn,
        *,
        groups: list[str],
        case_args: tuple[object, ...] = (),
        case_kwargs: CaseArgs | None = None,
    ) -> None:
        """Execute fixtures in correct order around *test_fn*.

        When *case_args* / *case_kwargs* are provided (from
        ``@test.cases``), ``case_args`` is splatted positionally and
        ``case_kwargs`` entries are merged into the kwargs passed to
        *test_fn* alongside the fixture-injected values. A collision
        between a case kwarg and a fixture parameter raises
        ``TypeError`` — case parameters may not shadow ``Depends()``
        parameters.
        """
        # Build the scope chain: [], ["a"], ["a", "b"], ...
        scopes: list[tuple[str, ...]] = [
            (),
            *[tuple(groups[: i + 1]) for i in range(len(groups))],
        ]

        self._visited_scopes.update(scopes)

        # Ordered setup list: outer→inner, definition order within each scope.
        # Collected so we can tear down in strict reverse.
        setup_sequence: list[_RegisteredFixture] = []

        for scope in scopes:
            scope_fixtures = sorted(
                (f for f in self._fixtures if tuple(f.groups) == scope),
                key=lambda f: f.line_number,
            )
            for f in scope_fixtures:
                if f.per == "scope":
                    key = _ScopeKey(id(f.fn), tuple(f.groups))
                    if key not in self._scope_initialized:
                        self._resolver.resolve_hook(f.fn)
                        self._scope_initialized.add(key)
                else:
                    setup_sequence.append(f)

        # Run per-test setup for this test.
        for f in setup_sequence:
            self._resolver.resolve_hook(f.fn)

        try:
            test_kwargs = self._resolver.resolve(test_fn)
            if case_kwargs:
                conflicts = set(test_kwargs).intersection(case_kwargs)
                if conflicts:
                    names = ", ".join(sorted(conflicts))
                    msg = (
                        f"@test.cases argument(s) {{{names}}} collide with "
                        f"fixture-injected parameter(s) of {test_fn.__name__}"
                    )
                    raise TypeError(msg)
                test_kwargs = {**test_kwargs, **case_kwargs}
            if inspect.iscoroutinefunction(test_fn):
                # Drive the test on the resolver's shared loop so the
                # test body awaits on the same loop that ran any async
                # fixture setup.
                self._resolver.shared_loop.run_until_complete(
                    test_fn(*case_args, **test_kwargs)
                )
            else:
                test_fn(*case_args, **test_kwargs)
        finally:
            # Teardown per-test generator fixtures in reverse setup order.
            self._resolver.teardown_test_generators()
            # Clear per-test cached values.
            self._resolver.clear_test_cache()

    def finalize(self) -> None:
        """Run per-scope teardown. Called by the worker after the last test."""
        self._resolver.teardown_scope_generators()
        self._resolver.clear_all()
        self._resolver.close_shared_loop()
