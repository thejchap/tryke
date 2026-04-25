"""Type-check sanity cases for ``Expectation.to_be_instance_of``.

The single-class overload binds the expectation's value type (``C``)
and requires ``cls: type[C]``. Covariance of ``type[...]`` means
subclasses of the expected type are accepted (downcast checks), while
unrelated types are rejected by the checker.

**Manual negative verification.** Each mutation below should be
rejected by ``ty check`` (and by mypy/pyright). Introduce one at a
time and run ``uvx prek run ty -a`` (or the type checker directly).
Revert before committing.

1. **Unrelated type** — change
   ``expect(42).to_be_instance_of(int)`` to
   ``expect(42).to_be_instance_of(str)``. Expected: ``type[str]`` is
   not assignable to ``type[int]``.

2. **Sibling subclass** — with
   ``derived_as_base: Base = Derived()``, change the matcher to
   ``expect(derived_as_base).to_be_instance_of(Sibling)`` where
   ``Sibling`` is a second subclass of ``Base`` not related to
   ``Derived``. Expected: rejected, because ``type[Sibling]`` is
   assignable to ``type[Base]`` (fine), but if the value is narrowed
   to ``Expectation[Derived]`` the checker flags the mismatch.

The tuple overload is intentionally permissive: mixing unrelated types
like ``(bytes, str)`` is a legitimate "any of these" runtime check
that can't be statically constrained without forcing callers to
pre-widen their value to the union.
"""

from __future__ import annotations

from tryke import describe, expect, test


class _Base:
    pass


class _Derived(_Base):
    pass


with describe("to_be_instance_of typing"):

    @test(name="single-class form accepts exact type")
    def single_class_exact() -> None:
        expect(42, "42 is int").to_be_instance_of(int)
        expect("hi", "'hi' is str").to_be_instance_of(str)

    @test(name="single-class form accepts covariant downcast")
    def single_class_downcast() -> None:
        value: _Base = _Derived()
        expect(value, "Base value is a Derived at runtime").to_be_instance_of(_Derived)

    @test(name="tuple form accepts unrelated types")
    def tuple_any_of() -> None:
        expect("hi", "str matches (bytes, str)").to_be_instance_of((bytes, str))
        expect(b"x", "bytes matches (bytes, str)").to_be_instance_of((bytes, str))
