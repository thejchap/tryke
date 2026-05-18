"""Shared test execution logic used by both the JSON-RPC worker and the
browser playground.

The core function is :func:`run_test`, which takes an already-resolved
callable and optional ``HookExecutor``, executes the test with fixture
injection, soft-assertion tracking, stdout/stderr capture, and xfail
handling, and returns a typed result dict that the Rust reporter can
deserialize.
"""

from __future__ import annotations

import asyncio
import contextlib
import inspect
import io
import sys
import time
import traceback
import unittest
from pathlib import Path
from typing import TYPE_CHECKING, Literal, NotRequired, TypedDict

from tryke.expect import (
    CaseArgs,
    CasesMarked,
    ExpectationError,
    SoftContext,
    SoftFailure,
    _set_soft_context,
    _SkipMarked,
    _TodoMarked,
    _XfailMarked,
)

if TYPE_CHECKING:
    from collections.abc import Generator

    from tryke.hooks import HookExecutor, _FixtureFn

_TRYKE_PKG = str(Path(__file__).resolve().parent)


# -- Wire-format TypedDicts (mirror crates/tryke_runner/src/protocol.rs) ------


class AssertionWire(TypedDict):
    expression: str
    expected: str
    received: str
    line: int
    column: NotRequired[int]
    file: NotRequired[str]


class PassedResult(TypedDict):
    outcome: Literal["passed"]
    duration_ms: int
    stdout: str
    stderr: str


class FailedResult(TypedDict):
    outcome: Literal["failed"]
    duration_ms: int
    message: str
    traceback: str | None
    assertions: list[AssertionWire]
    executed_lines: list[int]
    stdout: str
    stderr: str


class SkippedResult(TypedDict):
    outcome: Literal["skipped"]
    duration_ms: int
    reason: str | None
    stdout: str
    stderr: str


class XFailedResult(TypedDict):
    outcome: Literal["xfailed"]
    duration_ms: int
    reason: str | None
    stdout: str
    stderr: str


class XPassedResult(TypedDict):
    outcome: Literal["xpassed"]
    duration_ms: int
    stdout: str
    stderr: str


class TodoResult(TypedDict):
    outcome: Literal["todo"]
    duration_ms: int
    description: str | None
    stdout: str
    stderr: str


type TestResult = (
    PassedResult
    | FailedResult
    | SkippedResult
    | XFailedResult
    | XPassedResult
    | TodoResult
)


# -- Result constructors ------------------------------------------------------


def passed(
    duration_ms: int,
    stdout: str,
    stderr: str,
) -> PassedResult:
    return {
        "outcome": "passed",
        "duration_ms": duration_ms,
        "stdout": stdout,
        "stderr": stderr,
    }


def failed(  # noqa: PLR0913
    duration_ms: int,
    message: str,
    tb: str | None,
    assertions: list[AssertionWire],
    stdout: str,
    stderr: str,
    *,
    executed_lines: list[int] | None = None,
) -> FailedResult:
    return {
        "outcome": "failed",
        "duration_ms": duration_ms,
        "message": message,
        "traceback": tb,
        "assertions": assertions,
        "executed_lines": executed_lines if executed_lines is not None else [],
        "stdout": stdout,
        "stderr": stderr,
    }


def skipped(
    duration_ms: int,
    reason: str | None,
    stdout: str,
    stderr: str,
) -> SkippedResult:
    return {
        "outcome": "skipped",
        "duration_ms": duration_ms,
        "reason": reason,
        "stdout": stdout,
        "stderr": stderr,
    }


def xfailed(
    duration_ms: int,
    reason: str | None,
    stdout: str,
    stderr: str,
) -> XFailedResult:
    return {
        "outcome": "xfailed",
        "duration_ms": duration_ms,
        "reason": reason,
        "stdout": stdout,
        "stderr": stderr,
    }


def xpassed(
    duration_ms: int,
    stdout: str,
    stderr: str,
) -> XPassedResult:
    return {
        "outcome": "xpassed",
        "duration_ms": duration_ms,
        "stdout": stdout,
        "stderr": stderr,
    }


def todo(
    duration_ms: int,
    description: str | None,
    stdout: str,
    stderr: str,
) -> TodoResult:
    return {
        "outcome": "todo",
        "duration_ms": duration_ms,
        "description": description,
        "stdout": stdout,
        "stderr": stderr,
    }


# -- Assertion extraction ------------------------------------------------------


def _is_user_frame(frame: traceback.FrameSummary) -> bool:
    return not str(
        Path(frame.filename).resolve(),
    ).startswith(_TRYKE_PKG)


def _make_assertion_wire(
    *,
    expression: str,
    expected: str,
    received: str,
    frame: traceback.FrameSummary | None = None,
) -> AssertionWire:
    wire: AssertionWire = {
        "expression": expression,
        "expected": expected,
        "received": received,
        "line": frame.lineno if frame is not None and frame.lineno is not None else 0,
    }
    if frame is not None:
        if frame.colno is not None:
            wire["column"] = frame.colno
        wire["file"] = frame.filename
    return wire


def extract_soft_failures(
    failures: list[SoftFailure],
) -> list[AssertionWire]:
    return [
        _make_assertion_wire(
            expression=(frame.line or "").strip() if frame else "",
            expected=err.expected,
            received=err.received,
            frame=frame,
        )
        for err, frame in failures
    ]


def extract_single(exc: ExpectationError) -> AssertionWire:
    tb = sys.exc_info()[2]
    frames = traceback.extract_tb(tb)
    for frame in reversed(frames):
        if _is_user_frame(frame):
            return _make_assertion_wire(
                expression=(frame.line or "").strip(),
                expected=exc.expected,
                received=exc.received,
                frame=frame,
            )
    return _make_assertion_wire(
        expression="",
        expected=exc.expected,
        received=exc.received,
    )


def extract_assertions(
    exc: AssertionError,
) -> list[AssertionWire]:
    if not isinstance(exc, ExpectationError):
        return []
    tb = sys.exc_info()[2]
    frames = traceback.extract_tb(tb)
    for frame in reversed(frames):
        if _is_user_frame(frame):
            return [
                _make_assertion_wire(
                    expression=(frame.line or "").strip(),
                    expected=exc.expected,
                    received=exc.received,
                    frame=frame,
                )
            ]
    return []


# -- Soft-assertion context ----------------------------------------------------


@contextlib.contextmanager
def soft_assertion_context() -> Generator[SoftContext, None, None]:
    ctx = SoftContext()
    _set_soft_context(ctx)
    try:
        yield ctx
    finally:
        _set_soft_context(None)


# -- Core test runner ----------------------------------------------------------


def run_test(  # noqa: C901, PLR0911, PLR0912, PLR0915
    fn: _FixtureFn,
    *,
    executor: HookExecutor | None = None,
    xfail: str | None = None,
    groups: list[str] | None = None,
    case_label: str | None = None,
) -> TestResult:
    """Execute a single test function and return a typed result dict.

    Parameters
    ----------
    fn:
        The test function to run. Must already be resolved (i.e. the
        actual callable, not a string name).
    executor:
        Optional :class:`HookExecutor` for fixture injection. When
        provided, fixtures are resolved through the executor's
        dependency graph. When ``None``, fixtures are resolved directly
        via a temporary :class:`DependencyResolver`.
    xfail:
        If set, the test is expected to fail with this reason string.
    groups:
        Scope chain for fixture scoping (e.g. ``["describe", "sub"]``).
    case_label:
        Label of the parametrized case to run (from ``@test.cases``).
    """
    case_args: tuple[object, ...] = ()
    case_kwargs: CaseArgs | None = None
    if case_label is not None:
        if not isinstance(fn, CasesMarked):
            return failed(
                0,
                (
                    f"{fn.__name__} was dispatched with case_label="
                    f"{case_label!r} but has no __tryke_cases__ attribute"
                ),
                "",
                [],
                "",
                "",
            )
        cases = fn.__tryke_cases__
        entry = next((e for e in cases if e.label == case_label), None)
        if entry is None:
            known = sorted(e.label for e in cases)
            return failed(
                0,
                (
                    f"{fn.__name__} has no case labeled "
                    f"{case_label!r}; known cases: {known}"
                ),
                "",
                [],
                "",
                "",
            )
        case_args = entry.args
        case_kwargs = entry.kwargs

        if entry.skip is not None:
            return skipped(0, entry.skip, "", "")
        if entry.todo is not None:
            return todo(0, entry.todo, "", "")
        if entry.xfail is not None:
            xfail = entry.xfail

    if isinstance(fn, _SkipMarked):
        return skipped(0, fn.__tryke_skip__, "", "")

    if isinstance(fn, _TodoMarked):
        return todo(0, fn.__tryke_todo__, "", "")

    is_xfail = xfail is not None or isinstance(fn, _XfailMarked)
    xfail_reason = (
        xfail
        if xfail is not None
        else (fn.__tryke_xfail__ if isinstance(fn, _XfailMarked) else None)
    )

    stdout_buf = io.StringIO()
    stderr_buf = io.StringIO()
    start = time.monotonic()

    with soft_assertion_context() as ctx:
        try:
            with (
                contextlib.redirect_stdout(stdout_buf),
                contextlib.redirect_stderr(stderr_buf),
            ):
                if executor is not None:
                    executor.run_test(
                        fn,
                        groups=groups or [],
                        case_args=case_args,
                        case_kwargs=case_kwargs,
                    )
                else:
                    _run_without_executor(fn, case_args, case_kwargs)

            ms = int((time.monotonic() - start) * 1000)
            out = stdout_buf.getvalue()
            err = stderr_buf.getvalue()

            if ctx.failures:
                if is_xfail:
                    return xfailed(ms, xfail_reason, out, err)
                return failed(
                    ms,
                    "assertion failed",
                    "",
                    extract_soft_failures(ctx.failures),
                    out,
                    err,
                    executed_lines=list(ctx.executed_lines),
                )
            if is_xfail:
                return xpassed(ms, out, err)
            return passed(ms, out, err)

        except unittest.SkipTest as exc:
            ms = int((time.monotonic() - start) * 1000)
            return skipped(
                ms,
                str(exc),
                stdout_buf.getvalue(),
                stderr_buf.getvalue(),
            )

        except ExpectationError as exc:
            ms = int((time.monotonic() - start) * 1000)
            out = stdout_buf.getvalue()
            err = stderr_buf.getvalue()
            if is_xfail:
                return xfailed(ms, xfail_reason, out, err)
            assertions = extract_soft_failures(ctx.failures)
            assertions.append(extract_single(exc))
            return failed(
                ms,
                str(exc) or "assertion failed",
                traceback.format_exc(),
                assertions,
                out,
                err,
                executed_lines=list(ctx.executed_lines),
            )

        except AssertionError as exc:
            ms = int((time.monotonic() - start) * 1000)
            out = stdout_buf.getvalue()
            err = stderr_buf.getvalue()
            if is_xfail:
                return xfailed(ms, xfail_reason, out, err)
            return failed(
                ms,
                str(exc) or "assertion failed",
                traceback.format_exc(),
                extract_assertions(exc),
                out,
                err,
                executed_lines=list(ctx.executed_lines),
            )

        except Exception as exc:  # noqa: BLE001
            ms = int((time.monotonic() - start) * 1000)
            out = stdout_buf.getvalue()
            err = stderr_buf.getvalue()
            if is_xfail:
                return xfailed(ms, xfail_reason, out, err)
            return failed(
                ms,
                f"{type(exc).__name__}: {exc}",
                traceback.format_exc(),
                [],
                out,
                err,
                executed_lines=list(ctx.executed_lines),
            )


def _run_without_executor(
    fn: _FixtureFn,
    case_args: tuple[object, ...],
    case_kwargs: CaseArgs | None,
) -> None:
    """Execute a test function with ad-hoc fixture resolution.

    Used by the playground (no pre-registered executor). Creates a
    temporary :class:`DependencyResolver`, resolves ``Depends()``
    markers, runs the function, and tears down fixtures afterward.
    """
    from tryke.hooks import DependencyResolver  # noqa: PLC0415

    resolver = DependencyResolver()
    try:
        test_kwargs = resolver.resolve(fn)
        if case_kwargs:
            test_kwargs = {**test_kwargs, **case_kwargs}
        if inspect.iscoroutinefunction(fn):
            asyncio.run(fn(*case_args, **test_kwargs))
        else:
            fn(*case_args, **test_kwargs)
    finally:
        resolver.teardown_test_generators()
        resolver.teardown_scope_generators()
        resolver.clear_all()
        resolver.close_shared_loop()
