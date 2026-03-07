from __future__ import annotations

import contextlib
import importlib
import io
import json
import sys
import time
import traceback
import unittest
from pathlib import Path

from tryke.expect import ExpectationError

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
        return _run_test(params["module"], params["function"], modules)
    if method == "reload":
        return _reload(params.get("modules", []), modules)
    msg = f"unknown method: {method}"
    raise ValueError(msg)


def _run_test(module_name: str, function_name: str, modules: dict[str, object]) -> dict:
    if module_name not in modules:
        mod = importlib.import_module(module_name)
        modules[module_name] = mod
    else:
        mod = modules[module_name]

    fn = getattr(mod, function_name)

    stdout_buf = io.StringIO()
    stderr_buf = io.StringIO()
    start = time.monotonic()

    try:
        with (
            contextlib.redirect_stdout(stdout_buf),
            contextlib.redirect_stderr(stderr_buf),
        ):
            fn()
        duration_ms = int((time.monotonic() - start) * 1000)
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
    except AssertionError as exc:
        duration_ms = int((time.monotonic() - start) * 1000)
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
        return {
            "outcome": "failed",
            "message": f"{type(exc).__name__}: {exc}",
            "traceback": traceback.format_exc(),
            "assertions": [],
            "duration_ms": duration_ms,
            "stdout": stdout_buf.getvalue(),
            "stderr": stderr_buf.getvalue(),
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
