"""Thin test-execution harness for the browser playground.

Called by the Pyodide web worker. Imports user modules from the virtual
filesystem, runs each discovered test through :func:`tryke.runner.run_test`,
and returns JSON results in the flat runner wire format. The WASM
``format_results`` function handles conversion to the reporter's
internal representation.
"""

from __future__ import annotations

import contextlib
import doctest
import importlib
import io
import json
import sys
import time
import traceback
from pathlib import Path
from typing import TYPE_CHECKING, Any, Required, TypedDict

from tryke.hooks import _FIXTURE_ATTR, HookExecutor
from tryke.runner import TestResult, failed, passed, run_test

if TYPE_CHECKING:
    from types import ModuleType

    from tryke.expect import CasesMarked

_PYODIDE_ROOT = Path("/home/pyodide")

# Files written by previous run_tests() calls, relative to _PYODIDE_ROOT.
# Tracked so we can unlink and purge sys.modules entries for files the user
# removed from the playground between runs.
_WRITTEN_FILES: set[str] = set()


def _module_name(filename: str) -> str:
    """Map a filename to its importable dotted module name.

    Package ``__init__.py`` files map to the package itself
    (``pkg/__init__.py`` -> ``pkg``), matching how the import system
    caches them in ``sys.modules``.
    """
    mod = filename.removesuffix(".py").replace("/", ".")
    return mod.removesuffix(".__init__")


def _purge_module(name: str) -> None:
    """Drop *name* and any submodules from ``sys.modules``."""
    sys.modules.pop(name, None)
    prefix = f"{name}."
    for cached in [m for m in sys.modules if m.startswith(prefix)]:
        sys.modules.pop(cached, None)


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
    """
    if all_files_json:
        all_files: list[dict[str, str]] = json.loads(all_files_json)
        current_names = {f["name"] for f in all_files}

        for stale in _WRITTEN_FILES - current_names:
            stale_path = _PYODIDE_ROOT / stale
            with contextlib.suppress(FileNotFoundError):
                stale_path.unlink()
            _purge_module(_module_name(stale))

        _WRITTEN_FILES.clear()
        for f in all_files:
            file_path = _PYODIDE_ROOT / f["name"]
            file_path.parent.mkdir(parents=True, exist_ok=True)
            file_path.write_text(f["source"])
            _WRITTEN_FILES.add(f["name"])
            _purge_module(_module_name(f["name"]))
    else:
        file_path = _PYODIDE_ROOT / filename
        file_path.parent.mkdir(parents=True, exist_ok=True)
        file_path.write_text(source)

    module_name = _module_name(filename)
    _purge_module(module_name)
    importlib.invalidate_caches()
    return module_name


def _test_result(test: dict[str, Any], result: TestResult) -> dict[str, Any]:
    """Merge a test discovery item with its runner result."""
    return {"test": test, **result}


def _resolve_doctest_object(mod: object, object_path: str) -> object:
    """Walk a dotted attribute path off *mod* for doctest lookup."""
    obj = mod
    if object_path:
        for attr in object_path.split("."):
            obj = getattr(obj, attr)
    return obj


def _run_doctest(mod: object, object_path: str) -> TestResult:
    """Execute the doctest(s) for *object_path* on *mod*.

    Mirrors :meth:`tryke.worker.Worker._run_doctest` so playground
    results match the normal worker path for items that
    ``discover_file_from_source`` flags with ``doctest_object``.
    """
    try:
        obj = _resolve_doctest_object(mod, object_path)
    except Exception as exc:  # noqa: BLE001
        return failed(
            0,
            f"{type(exc).__name__}: {exc}",
            traceback.format_exc(),
            [],
            "",
            "",
        )

    finder = doctest.DocTestFinder(verbose=False, recurse=False)
    tests = finder.find(obj, name=object_path or getattr(mod, "__name__", ""))
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


class _HookInfo(TypedDict, total=False):
    """Shape of a hook item from WASM discovery (HookItem in TS).

    Fields with serde ``skip_serializing_if`` on the Rust side may be
    absent in the JSON, so everything except ``name`` is optional.
    """

    name: Required[str]
    per: str
    groups: list[str]
    line_number: int | None


def _build_executor(
    mod: ModuleType,
    hooks: list[_HookInfo],
) -> HookExecutor | None:
    """Create a :class:`HookExecutor` from WASM-discovered hook metadata.

    Returns ``None`` when no fixtures are found, so the runner falls back
    to the per-test ``DependencyResolver`` path (still correct for tests
    that only use ``Depends()`` without registered fixtures).
    """
    executor = HookExecutor()
    registered = False
    for hook in hooks:
        fn = getattr(mod, hook["name"], None)
        if fn is None or not hasattr(fn, _FIXTURE_ATTR):
            continue
        executor.register_fixture(
            fn,
            groups=hook.get("groups", []),
            line_number=hook.get("line_number") or 0,
        )
        registered = True
    return executor if registered else None


def run_tests(
    filename: str,
    source: str,
    tests_json: str,
    all_files_json: str | None = None,
    hooks_json: str | None = None,
) -> str:
    """Execute discovered tests and return JSON results for the WASM reporter."""
    tests: list[dict[str, Any]] = json.loads(tests_json)
    hooks: list[_HookInfo] = json.loads(hooks_json) if hooks_json else []
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
    executor = _build_executor(mod, hooks)

    try:
        for t in tests:
            doctest_object = t.get("doctest_object")
            if doctest_object is not None:
                results.append(_test_result(t, _run_doctest(mod, doctest_object)))
                continue

            name: str = t["name"]
            fn = getattr(mod, name, None)

            if fn is None:
                error = failed(0, f"function '{name}' not found", None, [], "", "")
                results.append(_test_result(t, error))
                continue

            case_label: str | None = None
            case_index = t.get("case_index")
            if case_index is not None:
                cases_marked: CasesMarked = fn
                if hasattr(cases_marked, "__tryke_cases__"):
                    cases = cases_marked.__tryke_cases__
                    if case_index < len(cases):
                        case_label = cases[case_index].label

            result = run_test(
                fn,
                executor=executor,
                xfail=t.get("xfail"),
                groups=t.get("groups"),
                case_label=case_label,
            )
            results.append(_test_result(t, result))
    finally:
        if executor is not None:
            executor.finalize()

    return json.dumps(results)
