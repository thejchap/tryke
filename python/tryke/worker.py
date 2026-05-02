"""Long-lived worker process driven by the Rust runner over JSON-RPC.

The runner spawns `python -m tryke.worker`, wires the worker's stdin and
stdout up to its own pipes, and sends newline-delimited JSON-RPC 2.0
requests. The worker keeps module imports and fixture state cached across
many `run_test` calls to avoid re-paying Python startup cost on every test.

## Request shapes

Every request is one line of JSON shaped like:

    {"jsonrpc": "2.0", "id": N, "method": "<method>", "params": {...}}

Supported methods:

- `ping` → `"pong"` (used by `WorkerPool::warm` to force process spawn)
- `register_hooks {module, hooks: [HookWire...]}` → `null`
- `finalize_hooks {module}` → `null`
- `run_test    {module, function, xfail?, groups?}` → tagged outcome dict
- `run_doctest {module, object_path}` → tagged outcome dict

The wire format mirrors `crates/tryke_runner/src/protocol.rs`. The
TypedDicts below (`_AssertionWire`, `_PassedResult`, …) are the result
shapes the runner decodes. The hook-related request shapes are handled
at the dispatch boundary in `_register_hooks` — the Rust side
statically discovered the fixture metadata with Ruff, so the worker
just trusts the incoming list and does not re-parse source.

## Hook lifecycle

1. Runner discovers `@fixture` functions and `Depends(...)` references
   statically, builds a `HookWire` list per module, and sends
   `register_hooks` before any test in that module.
2. Worker stores the raw list in `self._hook_metadata[module]` without
   importing the module. Collection stays cheap.
3. On the first `run_test` for a module, `_get_module` imports it and
   `_get_executor` walks the stored metadata, `getattr`-ing each name
   off the imported module to resolve real callables, then builds a
   `HookExecutor` that owns fixture instances, dependency order, and
   teardown callbacks. Result is cached in `self._executors[module]`.
4. After every test in a module has run, the runner sends
   `finalize_hooks` and the executor runs `per="scope"` teardown.
5. In watch/server mode, file changes do not reach the worker over the
   wire — the runner instead kills this subprocess and respawns it,
   replaying `register_hooks` on the fresh process. `importlib.reload`
   is not used; a clean interpreter is the only reliable way to drop
   classes and closures captured under the old definitions.
"""

from __future__ import annotations

import asyncio
import contextlib
import doctest
import importlib
import inspect
import io
import json
import logging
import os
import sys
import time
import traceback
import unittest
from pathlib import Path
from typing import TYPE_CHECKING, Literal, NotRequired, TypedDict

import tryke_guard
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

# Flip `tryke_guard.__TRYKE_TESTING__` on for this worker process. User
# modules imported later via `_get_module` do
# `from tryke_guard import __TRYKE_TESTING__`, which binds the (now-True)
# module attribute into their globals, causing their `if __TRYKE_TESTING__:`
# guards to execute.
#
# Process-local by construction: children spawned by user tests start with a
# fresh `tryke_guard` import that reads the env-var default (False). To opt a
# child into test mode, pass env={**os.environ, "TRYKE_TESTING": "1"}.
tryke_guard.__TRYKE_TESTING__ = True

if TYPE_CHECKING:
    from collections.abc import Generator
    from types import ModuleType
    from typing import TextIO

_TRYKE_PKG = str(Path(__file__).resolve().parent)

# Worker-side logger. Debug/trace output is useful when auditing module
# import and dispatch; enable with `TRYKE_WORKER_LOG=DEBUG` (or `TRACE`).
# Messages go to stderr so they don't corrupt the JSON-RPC channel on
# stdout. Off by default.
_log = logging.getLogger("tryke.worker")


# -- Wire-format TypedDicts (mirror crates/tryke_runner/src/protocol.rs) ------


class _AssertionWire(TypedDict):
    expression: str
    expected: str
    received: str
    line: NotRequired[int]
    file: NotRequired[str]


class _PassedResult(TypedDict):
    outcome: Literal["passed"]
    duration_ms: int
    stdout: str
    stderr: str


class _FailedResult(TypedDict):
    outcome: Literal["failed"]
    duration_ms: int
    message: str
    traceback: str | None
    assertions: list[_AssertionWire]
    executed_lines: list[int]
    stdout: str
    stderr: str


class _SkippedResult(TypedDict):
    outcome: Literal["skipped"]
    duration_ms: int
    reason: str | None
    stdout: str
    stderr: str


class _XFailedResult(TypedDict):
    outcome: Literal["xfailed"]
    duration_ms: int
    reason: str | None
    stdout: str
    stderr: str


class _XPassedResult(TypedDict):
    outcome: Literal["xpassed"]
    duration_ms: int
    stdout: str
    stderr: str


class _TodoResult(TypedDict):
    outcome: Literal["todo"]
    duration_ms: int
    description: str | None
    stdout: str
    stderr: str


type _TestResult = (
    _PassedResult
    | _FailedResult
    | _SkippedResult
    | _XFailedResult
    | _XPassedResult
    | _TodoResult
)

type _DispatchResult = _TestResult | str | None


class _InvalidParamsError(Exception):
    """Missing or invalid JSON-RPC method parameter."""


def _passed(
    duration_ms: int,
    stdout: str,
    stderr: str,
) -> _PassedResult:
    return {
        "outcome": "passed",
        "duration_ms": duration_ms,
        "stdout": stdout,
        "stderr": stderr,
    }


def _failed(  # noqa: PLR0913
    duration_ms: int,
    message: str,
    tb: str | None,
    assertions: list[_AssertionWire],
    stdout: str,
    stderr: str,
    *,
    executed_lines: list[int] | None = None,
) -> _FailedResult:
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


def _skipped(
    duration_ms: int,
    reason: str | None,
    stdout: str,
    stderr: str,
) -> _SkippedResult:
    return {
        "outcome": "skipped",
        "duration_ms": duration_ms,
        "reason": reason,
        "stdout": stdout,
        "stderr": stderr,
    }


def _xfailed(
    duration_ms: int,
    reason: str | None,
    stdout: str,
    stderr: str,
) -> _XFailedResult:
    return {
        "outcome": "xfailed",
        "duration_ms": duration_ms,
        "reason": reason,
        "stdout": stdout,
        "stderr": stderr,
    }


def _xpassed(
    duration_ms: int,
    stdout: str,
    stderr: str,
) -> _XPassedResult:
    return {
        "outcome": "xpassed",
        "duration_ms": duration_ms,
        "stdout": stdout,
        "stderr": stderr,
    }


def _todo(
    duration_ms: int,
    description: str | None,
    stdout: str,
    stderr: str,
) -> _TodoResult:
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
) -> _AssertionWire:
    wire: _AssertionWire = {
        "expression": expression,
        "expected": expected,
        "received": received,
    }
    if frame is not None:
        if frame.lineno is not None:
            wire["line"] = frame.lineno
        wire["file"] = frame.filename
    return wire


def _extract_soft_failures(
    failures: list[SoftFailure],
) -> list[_AssertionWire]:
    return [
        _make_assertion_wire(
            expression=(frame.line or "").strip() if frame else "",
            expected=err.expected,
            received=err.received,
            frame=frame,
        )
        for err, frame in failures
    ]


def _extract_single(exc: ExpectationError) -> _AssertionWire:
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


def _extract_assertions(
    exc: AssertionError,
) -> list[_AssertionWire]:
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


class Worker:
    def __init__(
        self,
        input_stream: TextIO,
        output_stream: TextIO,
    ) -> None:
        self._input = input_stream
        self._output = output_stream
        self._modules: dict[str, ModuleType] = {}
        # Hook metadata registered per module by the runner (from JSON-RPC).
        self._hook_metadata: dict[str, list[object]] = {}
        # Hook executors cached per module.
        self._executors: dict[str, HookExecutor] = {}

    def run(self) -> None:
        for raw in self._input:
            line = raw.strip()
            if not line:
                continue
            try:
                req = json.loads(line)
            except json.JSONDecodeError as exc:
                self._write(
                    {
                        "jsonrpc": "2.0",
                        "id": None,
                        "error": {
                            "code": -32700,
                            "message": str(exc),
                        },
                    }
                )
                continue

            id_ = req.get("id")
            method = req.get("method", "")
            params = req.get("params") or {}

            try:
                result = self._dispatch(method, params)
                self._write(
                    {
                        "jsonrpc": "2.0",
                        "id": id_,
                        "result": result,
                    }
                )
            except _InvalidParamsError as exc:
                self._write(
                    {
                        "jsonrpc": "2.0",
                        "id": id_,
                        "error": {
                            "code": -32602,
                            "message": str(exc),
                        },
                    }
                )
            except Exception as exc:  # noqa: BLE001
                self._write(
                    {
                        "jsonrpc": "2.0",
                        "id": id_,
                        "error": {
                            "code": -32603,
                            "message": str(exc),
                            "traceback": traceback.format_exc(),
                        },
                    }
                )

    def _write(self, obj: dict[str, object]) -> None:
        self._output.write(json.dumps(obj) + "\n")
        self._output.flush()

    def _require_str(
        self,
        params: dict[str, object],
        key: str,
        method: str,
    ) -> str:
        if key not in params:
            msg = f"method '{method}' requires parameter '{key}'"
            raise _InvalidParamsError(msg)
        value = params[key]
        if not isinstance(value, str):
            msg = (
                f"method '{method}' parameter '{key}'"
                f" must be a string, got {type(value).__name__}"
            )
            raise _InvalidParamsError(msg)
        return value

    def _dispatch(
        self,
        method: str,
        params: dict[str, object],
    ) -> _DispatchResult:
        if method == "ping":
            return "pong"
        if method == "register_hooks":
            return self._register_hooks(
                self._require_str(params, "module", method),
                params.get("hooks", []),
            )
        if method == "finalize_hooks":
            return self._finalize_hooks(
                self._require_str(params, "module", method),
            )
        if method == "run_test":
            xfail_raw = params.get("xfail")
            raw_groups = params.get("groups", [])
            groups = (
                [str(g) for g in raw_groups] if isinstance(raw_groups, list) else []
            )
            case_label_raw = params.get("case_label")
            case_label = str(case_label_raw) if case_label_raw is not None else None
            return self._run_test(
                self._require_str(params, "module", method),
                self._require_str(params, "function", method),
                xfail=(str(xfail_raw) if xfail_raw is not None else None),
                groups=groups,
                case_label=case_label,
            )
        if method == "run_doctest":
            return self._run_doctest(
                self._require_str(params, "module", method),
                str(params.get("object_path", "")),
            )
        msg = f"unknown method: {method}"
        raise ValueError(msg)

    def _get_module(self, module_name: str) -> ModuleType:
        if module_name not in self._modules:
            # Redirect both sys.stdout (Python-level) and fd 1 (C-level)
            # during import so that libraries which write to the real stdout
            # via cffi/ctypes (e.g. weasyprint) don't corrupt the json-rpc
            # channel.  Captured output is re-emitted on stderr instead.
            buf = io.StringIO()
            saved_fd = os.dup(1)
            os.dup2(2, 1)  # point fd 1 at stderr
            try:
                with contextlib.redirect_stdout(buf):
                    mod = importlib.import_module(module_name)
            finally:
                os.dup2(saved_fd, 1)
                os.close(saved_fd)
                captured = buf.getvalue()
                if captured:
                    sys.stderr.write(captured)
            self._modules[module_name] = mod
            return mod
        return self._modules[module_name]

    def _register_hooks(
        self,
        module_name: str,
        hooks: object,
    ) -> None:
        """Store statically-discovered hook metadata for a module.

        The runner calls this once per module before any `run_test` for
        that module, passing a list of ``HookWire`` dicts (see
        ``crates/tryke_runner/src/protocol.rs``). We deliberately do not
        import ``module_name`` here: importing is deferred to
        :meth:`_get_module` so that collection stays cheap and failed
        imports surface as test failures instead of crashing the worker.

        Any previously-cached :class:`HookExecutor` for this module is
        dropped so the next test rebuilds fixtures from the fresh
        metadata — this matters when the runner re-registers the same
        module (e.g. after a worker respawn during watch/server mode).
        """
        if not isinstance(hooks, list):
            return
        self._hook_metadata[module_name] = list(hooks)
        # Invalidate any cached executor for this module.
        self._executors.pop(module_name, None)
        _log.debug("register_hooks: module=%s hook_count=%d", module_name, len(hooks))

    def _finalize_hooks(self, module_name: str) -> None:
        """Run scope-level teardown for a module's `per="scope"` fixtures."""
        executor = self._executors.get(module_name)
        if executor is not None:
            executor.finalize()

    def _get_executor(self, module_name: str) -> HookExecutor | None:
        """Build (or return cached) HookExecutor for a module."""
        if module_name in self._executors:
            return self._executors[module_name]

        hook_meta = self._hook_metadata.get(module_name)
        if not hook_meta:
            return None

        mod = self._get_module(module_name)
        executor = HookExecutor()
        for entry in hook_meta:
            if not isinstance(entry, dict):
                continue
            # JSON-RPC delivers dict[str, object]; rebuild with typed comprehension.
            h: dict[str, object] = {str(k): v for k, v in entry.items()}
            name = str(h.get("name", ""))
            raw_groups = h.get("groups", [])
            groups = (
                [str(g) for g in raw_groups] if isinstance(raw_groups, list) else []
            )
            raw_ln = h.get("line_number", 0)
            line_number = raw_ln if isinstance(raw_ln, int) else 0
            fn = getattr(mod, name, None)
            if fn is not None and _fixture_per(fn) is not None:
                executor.register_fixture(fn, groups=groups, line_number=line_number)

        self._executors[module_name] = executor
        return executor

    @contextlib.contextmanager
    def _soft_assertion_context(
        self,
    ) -> Generator[SoftContext, None, None]:
        ctx = SoftContext()
        _set_soft_context(ctx)
        try:
            yield ctx
        finally:
            _set_soft_context(None)

    def _run_test(  # noqa: C901, PLR0911, PLR0912, PLR0915
        self,
        module_name: str,
        function_name: str,
        *,
        xfail: str | None = None,
        groups: list[str] | None = None,
        case_label: str | None = None,
    ) -> _TestResult:
        try:
            mod = self._get_module(module_name)
            fn = getattr(mod, function_name)
        except Exception as exc:  # noqa: BLE001
            return _failed(
                0,
                f"{type(exc).__name__}: {exc}",
                traceback.format_exc(),
                [],
                "",
                "",
            )

        case_args: tuple[object, ...] = ()
        case_kwargs: CaseArgs | None = None
        if case_label is not None:
            if not isinstance(fn, CasesMarked):
                return _failed(
                    0,
                    (
                        f"{function_name} was dispatched with case_label="
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
                return _failed(
                    0,
                    (
                        f"{function_name} has no case labeled "
                        f"{case_label!r}; known cases: {known}"
                    ),
                    "",
                    [],
                    "",
                    "",
                )
            case_args = entry.args
            case_kwargs = entry.kwargs

            # Per-case modifiers (runtime fallback for non-literal values
            # that static discovery could not extract).
            if entry.skip is not None:
                return _skipped(0, entry.skip, "", "")
            if entry.todo is not None:
                return _todo(0, entry.todo, "", "")
            if entry.xfail is not None:
                xfail = entry.xfail

        # Runtime skip/todo (handles skip_if resolved at import time)
        if isinstance(fn, _SkipMarked):
            return _skipped(0, fn.__tryke_skip__, "", "")

        if isinstance(fn, _TodoMarked):
            return _todo(0, fn.__tryke_todo__, "", "")

        is_xfail = xfail is not None or isinstance(fn, _XfailMarked)
        xfail_reason = (
            xfail
            if xfail is not None
            else (fn.__tryke_xfail__ if isinstance(fn, _XfailMarked) else None)
        )

        stdout_buf = io.StringIO()
        stderr_buf = io.StringIO()
        start = time.monotonic()

        with self._soft_assertion_context() as ctx:
            try:
                with (
                    contextlib.redirect_stdout(stdout_buf),
                    contextlib.redirect_stderr(stderr_buf),
                ):
                    executor = self._get_executor(module_name)
                    if executor is not None:
                        executor.run_test(
                            fn,
                            groups=groups or [],
                            case_args=case_args,
                            case_kwargs=case_kwargs,
                        )
                    elif inspect.iscoroutinefunction(fn):
                        asyncio.run(fn(*case_args, **(case_kwargs or {})))
                    else:
                        fn(*case_args, **(case_kwargs or {}))

                ms = int((time.monotonic() - start) * 1000)
                out = stdout_buf.getvalue()
                err = stderr_buf.getvalue()

                if ctx.failures:
                    if is_xfail:
                        return _xfailed(
                            ms,
                            xfail_reason,
                            out,
                            err,
                        )
                    return _failed(
                        ms,
                        "assertion failed",
                        "",
                        _extract_soft_failures(ctx.failures),
                        out,
                        err,
                        executed_lines=list(ctx.executed_lines),
                    )
                if is_xfail:
                    return _xpassed(ms, out, err)
                return _passed(ms, out, err)

            except unittest.SkipTest as exc:
                ms = int((time.monotonic() - start) * 1000)
                return _skipped(
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
                    return _xfailed(
                        ms,
                        xfail_reason,
                        out,
                        err,
                    )
                assertions = _extract_soft_failures(
                    ctx.failures,
                )
                assertions.append(_extract_single(exc))
                return _failed(
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
                    return _xfailed(
                        ms,
                        xfail_reason,
                        out,
                        err,
                    )
                return _failed(
                    ms,
                    str(exc) or "assertion failed",
                    traceback.format_exc(),
                    _extract_assertions(exc),
                    out,
                    err,
                    executed_lines=list(ctx.executed_lines),
                )

            except Exception as exc:  # noqa: BLE001
                ms = int((time.monotonic() - start) * 1000)
                out = stdout_buf.getvalue()
                err = stderr_buf.getvalue()
                if is_xfail:
                    return _xfailed(
                        ms,
                        xfail_reason,
                        out,
                        err,
                    )
                return _failed(
                    ms,
                    f"{type(exc).__name__}: {exc}",
                    traceback.format_exc(),
                    [],
                    out,
                    err,
                    executed_lines=list(ctx.executed_lines),
                )

    def _run_doctest(
        self,
        module_name: str,
        object_path: str,
    ) -> _TestResult:
        try:
            mod = self._get_module(module_name)

            # Resolve the target object whose docstring we want to test.
            obj = mod
            if object_path:
                for attr in object_path.split("."):
                    obj = getattr(obj, attr)
        except Exception as exc:  # noqa: BLE001
            return _failed(
                0,
                f"{type(exc).__name__}: {exc}",
                traceback.format_exc(),
                [],
                "",
                "",
            )

        finder = doctest.DocTestFinder(
            verbose=False,
            recurse=False,
        )
        tests = finder.find(
            obj,
            name=object_path or module_name,
        )

        output_buf = io.StringIO()
        stdout_buf = io.StringIO()
        stderr_buf = io.StringIO()

        runner = doctest.DocTestRunner(
            verbose=False,
            optionflags=doctest.ELLIPSIS,
        )

        start = time.monotonic()
        with (
            contextlib.redirect_stdout(stdout_buf),
            contextlib.redirect_stderr(stderr_buf),
        ):
            for dt in tests:
                runner.run(
                    dt,
                    out=output_buf.write,
                    clear_globs=False,
                )

        ms = int((time.monotonic() - start) * 1000)
        with contextlib.redirect_stdout(io.StringIO()):
            summary = runner.summarize(verbose=False)
        out = stdout_buf.getvalue()
        err = stderr_buf.getvalue()

        if summary.failed > 0:
            return _failed(
                ms,
                output_buf.getvalue(),
                None,
                [],
                out,
                err,
            )
        return _passed(ms, out, err)


def _configure_logging_from_env() -> None:
    """Opt-in worker logging via ``TRYKE_LOG``.

    Off by default so normal test runs don't emit anything on stderr.
    The rust runner sets ``TRYKE_LOG=<level>`` on the worker env when
    ``-v`` / ``-q`` or the ``TRYKE_LOG`` env var asks for cross-language
    verbosity, so users typically don't set this directly.

    Accepts ``DEBUG`` / ``INFO`` / ``WARN`` / ``ERROR`` / ``TRACE``.
    Output goes to stderr so it never contaminates the JSON-RPC stream
    on stdout.

    The legacy ``TRYKE_WORKER_LOG`` name is honored as a deprecated
    alias (one cycle) so existing shell exports keep working; a single
    deprecation line is logged when only the old name is set.
    """
    level_name = os.environ.get("TRYKE_LOG", "").strip().upper()
    legacy_name = os.environ.get("TRYKE_WORKER_LOG", "").strip().upper()
    use_legacy = not level_name and bool(legacy_name)
    if use_legacy:
        level_name = legacy_name
    if not level_name:
        return
    # Map TRACE to DEBUG since stdlib logging has no TRACE level.
    if level_name == "TRACE":
        level_name = "DEBUG"
    level = logging.getLevelName(level_name)
    if not isinstance(level, int):
        return
    handler = logging.StreamHandler(sys.stderr)
    handler.setFormatter(
        logging.Formatter("tryke.worker[%(process)d] %(levelname)s %(message)s")
    )
    _log.addHandler(handler)
    _log.setLevel(level)
    _log.propagate = False
    if use_legacy:
        _log.warning(
            "TRYKE_WORKER_LOG is deprecated; use TRYKE_LOG instead "
            "(it propagates to both rust and python workers)"
        )


def main() -> None:
    _configure_logging_from_env()
    _log.debug("worker main: starting (pid=%d)", os.getpid())
    Worker(sys.stdin, sys.stdout).run()


if __name__ == "__main__":
    main()
