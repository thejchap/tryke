from __future__ import annotations

import asyncio
import contextlib
import importlib
import inspect
import io
import json
import sys
import time
import traceback
import unittest
from pathlib import Path

from tryke.expect import ExpectationError

# Use sys.modules to get the actual module — `tryke.expect` the attribute
# is shadowed by the `expect` function re-exported in tryke/__init__.py.
_expect_mod = sys.modules["tryke.expect"]

_TRYKE_PKG = str(Path(__file__).resolve().parent)


def main() -> None:
    modules: dict[str, object] = {}
    for raw in sys.stdin:
        line = raw.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except json.JSONDecodeError as exc:
            _write(
                {
                    "jsonrpc": "2.0",
                    "id": None,
                    "error": {"code": -32700, "message": str(exc)},
                }
            )
            continue

        id_ = req.get("id")
        method = req.get("method", "")
        params = req.get("params") or {}

        try:
            result = _dispatch(method, params, modules)
            _write({"jsonrpc": "2.0", "id": id_, "result": result})
        except Exception as exc:  # noqa: BLE001
            _write(
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


def _write(obj: object) -> None:
    print(json.dumps(obj), flush=True)  # noqa: T201


def _dispatch(method: str, params: dict, modules: dict[str, object]) -> object:
    if method == "ping":
        return "pong"
    if method == "run_test":
        return _run_test(
            params["module"],
            params["function"],
            modules,
            xfail=params.get("xfail"),
        )
    if method == "reload":
        return _reload(params.get("modules", []), modules)
    msg = f"unknown method: {method}"
    raise ValueError(msg)


def _run_test(  # noqa: C901, PLR0911, PLR0912
    module_name: str,
    function_name: str,
    modules: dict[str, object],
    *,
    xfail: str | None = None,
) -> dict:
    if module_name not in modules:
        mod = importlib.import_module(module_name)
        modules[module_name] = mod
    else:
        mod = modules[module_name]

    fn = getattr(mod, function_name)

    # Runtime skip/todo checks (handles skip_if resolved at import time)
    if hasattr(fn, "__tryke_skip__"):
        return {
            "outcome": "skipped",
            "reason": fn.__tryke_skip__,
            "duration_ms": 0,
            "stdout": "",
            "stderr": "",
        }

    if hasattr(fn, "__tryke_todo__"):
        return {
            "outcome": "todo",
            "description": fn.__tryke_todo__,
            "duration_ms": 0,
            "stdout": "",
            "stderr": "",
        }

    # Determine if xfail — from wire param or runtime attribute
    is_xfail = xfail is not None or hasattr(fn, "__tryke_xfail__")
    xfail_reason = xfail if xfail is not None else getattr(fn, "__tryke_xfail__", None)

    stdout_buf = io.StringIO()
    stderr_buf = io.StringIO()
    start = time.monotonic()

    ctx = _expect_mod.SoftContext()
    _expect_mod._soft_context = ctx  # noqa: SLF001
    try:
        with (
            contextlib.redirect_stdout(stdout_buf),
            contextlib.redirect_stderr(stderr_buf),
        ):
            if inspect.iscoroutinefunction(fn):
                asyncio.run(fn())
            else:
                fn()
        if ctx.failures:
            duration_ms = int((time.monotonic() - start) * 1000)
            if is_xfail:
                return {
                    "outcome": "xfailed",
                    "reason": xfail_reason or None,
                    "duration_ms": duration_ms,
                    "stdout": stdout_buf.getvalue(),
                    "stderr": stderr_buf.getvalue(),
                }
            assertions = _extract_soft_failures(ctx.failures)
            return {
                "outcome": "failed",
                "message": "assertion failed",
                "traceback": "",
                "assertions": assertions,
                "duration_ms": duration_ms,
                "stdout": stdout_buf.getvalue(),
                "stderr": stderr_buf.getvalue(),
            }
        duration_ms = int((time.monotonic() - start) * 1000)
        if is_xfail:
            # Test passed but was expected to fail → xpassed (hard failure)
            return {
                "outcome": "xpassed",
                "duration_ms": duration_ms,
                "stdout": stdout_buf.getvalue(),
                "stderr": stderr_buf.getvalue(),
            }
        return {
            "outcome": "passed",
            "duration_ms": duration_ms,
            "stdout": stdout_buf.getvalue(),
            "stderr": stderr_buf.getvalue(),
        }
    except unittest.SkipTest as exc:
        duration_ms = int((time.monotonic() - start) * 1000)
        return {
            "outcome": "skipped",
            "duration_ms": duration_ms,
            "reason": str(exc),
            "stdout": stdout_buf.getvalue(),
            "stderr": stderr_buf.getvalue(),
        }
    except ExpectationError as exc:
        duration_ms = int((time.monotonic() - start) * 1000)
        if is_xfail:
            return {
                "outcome": "xfailed",
                "reason": xfail_reason or None,
                "duration_ms": duration_ms,
                "stdout": stdout_buf.getvalue(),
                "stderr": stderr_buf.getvalue(),
            }
        # .fatal() raised — include it plus any prior soft failures
        assertions = _extract_soft_failures(ctx.failures)
        assertions.append(_extract_single(exc))
        return {
            "outcome": "failed",
            "message": str(exc) or "assertion failed",
            "traceback": traceback.format_exc(),
            "assertions": assertions,
            "duration_ms": duration_ms,
            "stdout": stdout_buf.getvalue(),
            "stderr": stderr_buf.getvalue(),
        }
    except AssertionError as exc:
        duration_ms = int((time.monotonic() - start) * 1000)
        if is_xfail:
            return {
                "outcome": "xfailed",
                "reason": xfail_reason or None,
                "duration_ms": duration_ms,
                "stdout": stdout_buf.getvalue(),
                "stderr": stderr_buf.getvalue(),
            }
        assertions = _extract_assertions(exc)
        return {
            "outcome": "failed",
            "message": str(exc) or "assertion failed",
            "traceback": traceback.format_exc(),
            "assertions": assertions,
            "duration_ms": duration_ms,
            "stdout": stdout_buf.getvalue(),
            "stderr": stderr_buf.getvalue(),
        }
    except Exception as exc:  # noqa: BLE001
        duration_ms = int((time.monotonic() - start) * 1000)
        if is_xfail:
            return {
                "outcome": "xfailed",
                "reason": xfail_reason or None,
                "duration_ms": duration_ms,
                "stdout": stdout_buf.getvalue(),
                "stderr": stderr_buf.getvalue(),
            }
        return {
            "outcome": "failed",
            "message": f"{type(exc).__name__}: {exc}",
            "traceback": traceback.format_exc(),
            "assertions": [],
            "duration_ms": duration_ms,
            "stdout": stdout_buf.getvalue(),
            "stderr": stderr_buf.getvalue(),
        }
    finally:
        _expect_mod._soft_context = None  # noqa: SLF001


def _extract_soft_failures(
    failures: list[tuple[ExpectationError, traceback.FrameSummary | None]],
) -> list[dict]:
    result = []
    for err, frame in failures:
        entry: dict = {
            "expression": "",
            "expected": err.expected,
            "received": err.received,
        }
        if frame is not None:
            entry["expression"] = (frame.line or "").strip()
            entry["line"] = frame.lineno
            entry["file"] = frame.filename
        result.append(entry)
    return result


def _extract_single(exc: ExpectationError) -> dict:
    tb = sys.exc_info()[2]
    frames = traceback.extract_tb(tb)
    for frame in reversed(frames):
        if _is_user_frame(frame):
            return {
                "expression": (frame.line or "").strip(),
                "expected": exc.expected,
                "received": exc.received,
                "line": frame.lineno,
                "file": frame.filename,
            }
    return {
        "expression": "",
        "expected": exc.expected,
        "received": exc.received,
    }


def _is_user_frame(frame: traceback.FrameSummary) -> bool:
    return not str(Path(frame.filename).resolve()).startswith(_TRYKE_PKG)


def _extract_assertions(exc: AssertionError) -> list[dict]:
    if not isinstance(exc, ExpectationError):
        return []
    tb = sys.exc_info()[2]
    frames = traceback.extract_tb(tb)
    for frame in reversed(frames):
        if _is_user_frame(frame):
            return [
                {
                    "expression": (frame.line or "").strip(),
                    "expected": exc.expected,
                    "received": exc.received,
                    "line": frame.lineno,
                    "file": frame.filename,
                }
            ]
    return []


def _reload(module_names: list[str], modules: dict[str, object]) -> None:
    for name in module_names:
        if name in sys.modules:
            reloaded = importlib.reload(sys.modules[name])
            modules[name] = reloaded


if __name__ == "__main__":
    main()
