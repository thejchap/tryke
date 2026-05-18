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
import sys
from typing import TYPE_CHECKING, Any

from tryke.runner import TestResult, failed, run_test

if TYPE_CHECKING:
    from tryke.expect import CasesMarked


def _write_files(
    filename: str,
    source: str,
    all_files_json: str | None,
) -> str:
    """Write user source files to the virtual FS and return the module name."""
    if all_files_json:
        all_files: list[dict[str, str]] = json.loads(all_files_json)
        for f in all_files:
            with open(f"/home/pyodide/{f['name']}", "w") as fh:  # noqa: PTH123
                fh.write(f["source"])
            mod_name = f["name"].replace(".py", "").replace("/", ".")
            sys.modules.pop(mod_name, None)
    else:
        with open(f"/home/pyodide/{filename}", "w") as fh:  # noqa: PTH123
            fh.write(source)

    module_name = filename.replace(".py", "").replace("/", ".")
    sys.modules.pop(module_name, None)
    return module_name


def _test_result(test: dict[str, Any], result: TestResult) -> dict[str, Any]:
    """Merge a test discovery item with its runner result."""
    return {"test": test, **result}


def run_tests(
    filename: str,
    source: str,
    tests_json: str,
    all_files_json: str | None = None,
) -> str:
    """Execute discovered tests and return JSON results for the WASM reporter."""
    tests: list[dict[str, Any]] = json.loads(tests_json)
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

    for t in tests:
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

        result = run_test(fn, xfail=t.get("xfail"), case_label=case_label)
        results.append(_test_result(t, result))

    return json.dumps(results)
