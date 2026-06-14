"""Shared test execution logic used by both the JSON-RPC worker and the
browser playground.

The core function is :func:`run_test`, which takes an already-resolved
callable and a ``HookExecutor``, executes the test with fixture injection,
soft-assertion tracking, stdout/stderr capture, and xfail handling, and
returns a typed result dict that the Rust reporter can deserialize.
"""

from __future__ import annotations

import contextlib
import doctest
import io
import sys
import time
import traceback
import unittest
from pathlib import Path
from typing import TYPE_CHECKING, Literal, NotRequired, Required, TypedDict

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
from tryke.hooks import HookExecutor, _fixture_per

if TYPE_CHECKING:
    from collections.abc import Generator, Iterable
    from types import ModuleType

    from tryke.hooks import _FixtureFn

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


class HookInfo(TypedDict, total=False):
    """Statically-discovered fixture metadata, shared by the worker and playground.

    Fields with serde ``skip_serializing_if`` on the Rust side may be
    absent in the JSON, so everything except ``name`` is optional.
    """

    name: Required[str]
    per: str
    groups: list[str]
    line_number: int | None


def build_executor_from_hooks(
    mod: ModuleType,
    hooks: Iterable[HookInfo],
) -> HookExecutor:
    """Build a :class:`HookExecutor` from statically-discovered hook metadata.

    Walks *hooks*, looks up each named callable on *mod*, and registers
    the ones decorated with ``@fixture``. The returned executor is used
    even when no fixtures were registered so all ``Depends()`` resolution
    goes through one lifecycle path.
    """
    executor = HookExecutor()
    for hook in hooks:
        fn = getattr(mod, hook["name"], None)
        if fn is None or _fixture_per(fn) is None:
            continue
        executor.register_fixture(
            fn,
            groups=hook.get("groups") or [],
            line_number=hook.get("line_number") or 0,
        )
    return executor


def run_doctest(mod: object, object_path: str) -> TestResult:
    """Execute the doctest(s) for *object_path* on *mod*.

    Walks ``object_path`` off *mod* (e.g. ``"Foo.bar"``), finds all
    contained DocTests, and runs them with stdout/stderr captured.
    Returns a failed result if any examples failed, otherwise passed.
    """
    try:
        obj = mod
        if object_path:
            for attr in object_path.split("."):
                obj = getattr(obj, attr)
    except Exception as exc:  # noqa: BLE001
        return failed(
            0,
            f"{type(exc).__name__}: {exc}",
            traceback.format_exc(),
            [],
            "",
            "",
        )

    finder_name = object_path or getattr(mod, "__name__", "")
    finder = doctest.DocTestFinder(verbose=False, recurse=False)
    tests = finder.find(obj, name=finder_name)
    output_buf = io.StringIO()
    stdout_buf = io.StringIO()
    stderr_buf = io.StringIO()
    runner = doctest.DocTestRunner(verbose=False, optionflags=doctest.ELLIPSIS)

    start = time.monotonic()
    with (
        contextlib.redirect_stdout(stdout_buf),
        contextlib.redirect_stderr(stderr_buf),
    ):
        for dt in tests:
            runner.run(dt, out=output_buf.write, clear_globs=False)

    ms = int((time.monotonic() - start) * 1000)
    with contextlib.redirect_stdout(io.StringIO()):
        summary = runner.summarize(verbose=False)
    out = stdout_buf.getvalue()
    err = stderr_buf.getvalue()

    if summary.failed > 0:
        return failed(ms, output_buf.getvalue(), None, [], out, err)
    return passed(ms, out, err)


@contextlib.contextmanager
def soft_assertion_context() -> Generator[SoftContext, None, None]:
    ctx = SoftContext()
    _set_soft_context(ctx)
    try:
        yield ctx
    finally:
        _set_soft_context(None)


def run_test(  # noqa: C901, PLR0911, PLR0912, PLR0915
    fn: _FixtureFn,
    *,
    executor: HookExecutor,
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
        :class:`HookExecutor` for fixture injection. It may be empty,
        but it still owns the resolver lifecycle for the whole run.
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
                executor.run_test(
                    fn,
                    groups=groups or [],
                    case_args=case_args,
                    case_kwargs=case_kwargs,
                )

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
