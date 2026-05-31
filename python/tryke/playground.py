"""Thin test-execution harness for the browser playground.

Called by the Pyodide web worker. Imports user modules from the virtual
filesystem, runs each discovered test through :func:`tryke.runner.run_test`,
and returns JSON results in the flat runner wire format. The WASM
``format_results`` function handles conversion to the reporter's
internal representation.
"""

from __future__ import annotations

import contextlib
import importlib
import json
import sys
from pathlib import Path
from typing import Any

from tryke.runner import (
    HookInfo,
    TestResult,
    build_executor_from_hooks,
    failed,
    run_doctest,
    run_test,
)

_PYODIDE_ROOT = Path("/home/pyodide")

# Names that would shadow the bundled tryke package or escape the sandbox.
_RESERVED_NAMES = frozenset({"tryke.py", "tryke_guard.py"})
_RESERVED_PREFIXES = ("tryke/", "tryke_guard/")

# Files written by previous run_tests() calls, relative to _PYODIDE_ROOT.
# Tracked so we can unlink and purge sys.modules entries for files the user
# removed from the playground between runs.
_WRITTEN_FILES: set[str] = set()


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


def _purge_module(name: str) -> None:
    """Drop *name* and any submodules from ``sys.modules``.

    Also clears the matching attribute on the parent package. The import
    system binds an imported submodule as an attribute on its parent
    (``pkg.helpers`` becomes ``pkg.helpers``), and that binding outlives a
    bare ``sys.modules`` pop. Without clearing it, a later
    ``from pkg import helpers`` resolves the stale attribute and runs code
    the user has since removed from the playground.
    """
    sys.modules.pop(name, None)
    prefix = f"{name}."
    for cached in [m for m in sys.modules if m.startswith(prefix)]:
        sys.modules.pop(cached, None)

    parent_name, _, child = name.rpartition(".")
    if parent_name and (parent := sys.modules.get(parent_name)) is not None:
        with contextlib.suppress(AttributeError):
            delattr(parent, child)


def _write_files(
    filename: str,
    source: str,
    all_files_json: str | None,
) -> str:
    """Write user source files to the virtual FS and return the module name.

    Creates parent directories for nested paths (e.g. ``pkg/helpers.py``),
    removes files left over from earlier runs whose tab the user has since
    closed, and invalidates Python's import caches so the next import sees
    the freshly-written tree.

    Rejects unsafe filenames instead of skipping them: ``run_tests`` still
    discovers and imports the active tab's module, so silently dropping an
    unsafe name would leave the runner importing the bundled ``tryke``
    package (or a never-written module) and reporting misleading errors.
    """
    if not _is_safe_filename(filename):
        msg = f"unsafe filename: {filename!r}"
        raise ValueError(msg)

    if all_files_json:
        all_files: list[dict[str, str]] = json.loads(all_files_json)
        current_names = {f["name"] for f in all_files}
        for name in current_names:
            if not _is_safe_filename(name):
                msg = f"unsafe filename: {name!r}"
                raise ValueError(msg)

        for stale in _WRITTEN_FILES - current_names:
            stale_path = _PYODIDE_ROOT / stale
            with contextlib.suppress(FileNotFoundError):
                stale_path.unlink()
            _purge_module(_module_name(stale))

        _WRITTEN_FILES.clear()
        for f in all_files:
            name = f["name"]
            file_path = _PYODIDE_ROOT / name
            file_path.parent.mkdir(parents=True, exist_ok=True)
            file_path.write_text(f["source"])
            _WRITTEN_FILES.add(name)
            _purge_module(_module_name(name))
    else:
        file_path = _PYODIDE_ROOT / filename
        file_path.parent.mkdir(parents=True, exist_ok=True)
        file_path.write_text(source)
        # Track single-file writes too so a later multi-file run that omits
        # this file treats it as stale and cleans it up.
        _WRITTEN_FILES.add(filename)

    module_name = _module_name(filename)
    _purge_module(module_name)
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
