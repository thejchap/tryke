"""Thin test-execution harness for the browser playground.

Called by the Pyodide web worker. Imports user modules from the virtual
filesystem, runs each discovered test through :func:`tryke.runner.run_test`,
and returns JSON results in the flat runner wire format. The WASM
``format_results`` function handles conversion to the reporter's
internal representation.
"""

from __future__ import annotations

import importlib
import json
import shutil
import sys
from pathlib import Path
from typing import Any, TypedDict

from tryke.runner import (
    HookInfo,
    TestResult,
    build_executor_from_hooks,
    failed,
    run_doctest,
    run_test,
)

_PYODIDE_ROOT = Path("/home/pyodide")
_USER_ROOT_NAME = "user"

# Names that would shadow the bundled tryke package or escape the sandbox.
_RESERVED_NAMES = frozenset({"tryke.py", "tryke_guard.py"})
_RESERVED_PREFIXES = ("tryke/", "tryke_guard/")


class SourceFile(TypedDict):
    name: str
    source: str


def _user_root() -> Path:
    return _PYODIDE_ROOT / _USER_ROOT_NAME


def _is_safe_filename(name: str) -> bool:
    """Reject filenames that could shadow tryke internals or escape the sandbox."""
    if ".." in name or name.startswith("/"):
        return False
    if name in _RESERVED_NAMES:
        return False
    return not any(name.startswith(p) for p in _RESERVED_PREFIXES)


def _module_name(filename: str) -> str:
    """Map a filename to its importable dotted module name.

    Package ``__init__.py`` files map to the package itself
    (``pkg/__init__.py`` -> ``pkg``), matching how the import system
    caches them in ``sys.modules``.
    """
    mod = filename.removesuffix(".py").replace("/", ".")
    return mod.removesuffix(".__init__")


def _write_files(
    filename: str,
    source: str,
    all_files_json: str | None,
) -> str:
    """Write user source files to the virtual FS and return the module name.

    Creates parent directories for nested paths (e.g. ``pkg/helpers.py``),
    resets the user-file sandbox, and invalidates Python's import caches so
    the next import sees the freshly-written tree.

    Rejects unsafe filenames instead of skipping them: ``run_tests`` still
    discovers and imports the active tab's module, so silently dropping an
    unsafe name would leave the runner importing the bundled ``tryke``
    package (or a never-written module) and reporting misleading errors.
    """
    if not _is_safe_filename(filename):
        msg = f"unsafe filename: {filename!r}"
        raise ValueError(msg)

    files: list[SourceFile] = (
        json.loads(all_files_json)
        if all_files_json
        else [{"name": filename, "source": source}]
    )
    for f in files:
        if not _is_safe_filename(f["name"]):
            msg = f"unsafe filename: {f['name']!r}"
            raise ValueError(msg)

    user_root = _user_root()
    shutil.rmtree(user_root, ignore_errors=True)
    user_root.mkdir(parents=True, exist_ok=True)

    for f in files:
        file_path = user_root / f["name"]
        file_path.parent.mkdir(parents=True, exist_ok=True)
        file_path.write_text(f["source"])

    module_name = _module_name(filename)
    importlib.invalidate_caches()
    return module_name


def _test_result(test: dict[str, Any], result: TestResult) -> dict[str, Any]:
    """Merge a test discovery item with its runner result."""
    return {"test": test, **result}


def run_tests(
    filename: str,
    source: str,
    tests_json: str,
    all_files_json: str | None = None,
    hooks_json: str | None = None,
) -> str:
    """Execute discovered tests and return JSON results for the WASM reporter."""
    tests: list[dict[str, Any]] = json.loads(tests_json)
    hooks: list[HookInfo] = json.loads(hooks_json) if hooks_json else []
    results: list[dict[str, Any]] = []

    module_name = _write_files(filename, source, all_files_json)

    if "tryke" not in sys.modules:
        import tryke  # noqa: F401, PLC0415

    try:
        mod = importlib.import_module(module_name)
    except Exception as exc:  # noqa: BLE001
        error = failed(0, f"{type(exc).__name__}: {exc}", None, [], "", "")
        results.extend(_test_result(t, error) for t in tests)
        return json.dumps(results)

    # Build a shared executor so per-scope fixtures are resolved once and
    # reused across all tests (matching the native worker's lifecycle).
    executor = build_executor_from_hooks(mod, hooks)

    try:
        for t in tests:
            doctest_object = t.get("doctest_object")
            if doctest_object is not None:
                results.append(_test_result(t, run_doctest(mod, doctest_object)))
                continue

            name: str = t["name"]
            fn = getattr(mod, name, None)

            if fn is None:
                error = failed(0, f"function '{name}' not found", None, [], "", "")
                results.append(_test_result(t, error))
                continue

            result = run_test(
                fn,
                executor=executor,
                xfail=t.get("xfail"),
                groups=t.get("groups"),
                case_label=t.get("case_label"),
            )
            results.append(_test_result(t, result))
    finally:
        executor.finalize()

    return json.dumps(results)
