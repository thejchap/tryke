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
result TypedDicts live in :mod:`tryke.runner` and are shared with the
playground. The hook-related request shapes are handled
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

import contextlib
import importlib
import io
import json
import logging
import os
import sys
import traceback
from typing import TYPE_CHECKING

import tryke_guard
from tryke.hooks import HookExecutor
from tryke.runner import (
    HookInfo,
    TestResult,
    build_executor_from_hooks,
    failed,
    run_doctest,
    run_test,
)

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
    from types import ModuleType
    from typing import TextIO

_log = logging.getLogger("tryke.worker")

type _DispatchResult = TestResult | str | None


class _InvalidParamsError(Exception):
    """Missing or invalid JSON-RPC method parameter."""


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
        self._hook_metadata: dict[str, list[HookInfo]] = {}
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
        typed: list[HookInfo] = []
        for entry in hooks:
            if not isinstance(entry, dict):
                continue
            h: dict[str, object] = {str(k): v for k, v in entry.items()}
            name_val = h.get("name")
            if not isinstance(name_val, str):
                continue
            raw_groups = h.get("groups", [])
            groups = (
                [str(g) for g in raw_groups] if isinstance(raw_groups, list) else []
            )
            raw_ln = h.get("line_number", 0)
            line_number = raw_ln if isinstance(raw_ln, int) else 0
            typed.append(
                {"name": name_val, "groups": groups, "line_number": line_number}
            )
        self._hook_metadata[module_name] = typed
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
        # Keep the executor entry even when no fixtures were registered so a
        # repeated lookup short-circuits via the cache hit above. The empty
        # executor still routes Depends() resolution through the shared
        # resolver, matching pre-extraction behavior.
        executor = build_executor_from_hooks(mod, hook_meta) or HookExecutor()
        self._executors[module_name] = executor
        return executor

    def _run_test(
        self,
        module_name: str,
        function_name: str,
        *,
        xfail: str | None = None,
        groups: list[str] | None = None,
        case_label: str | None = None,
    ) -> TestResult:
        try:
            mod = self._get_module(module_name)
            fn = getattr(mod, function_name)
        except Exception as exc:  # noqa: BLE001
            return failed(
                0,
                f"{type(exc).__name__}: {exc}",
                traceback.format_exc(),
                [],
                "",
                "",
            )

        return run_test(
            fn,
            executor=self._get_executor(module_name),
            xfail=xfail,
            groups=groups,
            case_label=case_label,
        )

    def _run_doctest(
        self,
        module_name: str,
        object_path: str,
    ) -> TestResult:
        try:
            mod = self._get_module(module_name)
        except Exception as exc:  # noqa: BLE001
            return failed(
                0,
                f"{type(exc).__name__}: {exc}",
                traceback.format_exc(),
                [],
                "",
                "",
            )
        return run_doctest(mod, object_path)


def _configure_logging_from_env() -> None:
    """Opt-in worker logging via ``TRYKE_LOG``.

    Off by default so normal test runs don't emit anything on stderr.
    The rust runner sets ``TRYKE_LOG=<level>`` on the worker env when
    ``-v`` (or ``TRYKE_LOG``) asks for cross-language verbosity, so
    users typically don't set this directly. ``-q``/quiet does not
    light up workers — workers stay silent unless the user explicitly
    asked for more verbosity than the rust default ``warn``.

    Accepts ``DEBUG`` / ``INFO`` / ``WARN`` / ``ERROR`` / ``TRACE``.
    Output goes to stderr so it never contaminates the JSON-RPC stream
    on stdout.
    """
    level_name = os.environ.get("TRYKE_LOG", "").strip().upper()
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


def main() -> None:
    _configure_logging_from_env()
    _log.debug("worker main: starting (pid=%d)", os.getpid())
    Worker(sys.stdin, sys.stdout).run()


if __name__ == "__main__":
    main()
