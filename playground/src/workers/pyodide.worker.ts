/// <reference lib="webworker" />

// @ts-expect-error pyodide loaded from CDN
import { loadPyodide, type PyodideInterface } from "https://cdn.jsdelivr.net/pyodide/v0.27.6/full/pyodide.mjs";

import initSource from "../../../python/tryke/__init__.py?raw";
import expectSource from "../../../python/tryke/expect.py?raw";
import hooksSource from "../../../python/tryke/hooks.py?raw";
import guardSource from "../../../python/tryke_guard.py?raw";

let pyodide: PyodideInterface | null = null;

const RUNNER = `
import json
import sys
import time
import traceback
import importlib


def _extract_soft_failures(failures):
    """Convert SoftFailure list to wire-format assertion dicts."""
    result = []
    for err, frame in failures:
        result.append({
            "expression": (frame.line or "").strip() if frame else "",
            "file": frame.filename if frame else None,
            "line": frame.lineno if frame else 0,
            "span_offset": getattr(err, "span_offset", 0),
            "span_length": getattr(err, "span_length", 0),
            "expected": str(getattr(err, "expected", "")),
            "received": str(getattr(err, "received", "")),
        })
    return result


def run_tests(filename, source, tests_json):
    from tryke.expect import (
        SoftContext,
        _set_soft_context,
        ExpectationError,
    )

    tests = json.loads(tests_json)
    results = []

    module_name = filename.replace(".py", "").replace("/", ".")
    with open(f"/home/pyodide/{filename}", "w") as f:
        f.write(source)

    if module_name in sys.modules:
        del sys.modules[module_name]

    if "tryke" not in sys.modules:
        import tryke

    try:
        mod = importlib.import_module(module_name)
    except Exception as exc:
        for t in tests:
            results.append({
                "test": t,
                "outcome": {
                    "status": "error",
                    "detail": f"{type(exc).__name__}: {exc}"
                },
                "duration": {"secs": 0, "nanos": 0},
                "stdout": "",
                "stderr": "",
            })
        return json.dumps(results)

    for t in tests:
        name = t["name"]
        fn = getattr(mod, name, None)
        case_index = t.get("case_index")

        if fn is None:
            results.append({
                "test": t,
                "outcome": {"status": "error", "detail": f"Function '{name}' not found"},
                "duration": {"secs": 0, "nanos": 0},
                "stdout": "",
                "stderr": "",
            })
            continue

        skip = t.get("skip")
        if skip is not None:
            results.append({
                "test": t,
                "outcome": {"status": "skipped", "detail": {"reason": skip or None}},
                "duration": {"secs": 0, "nanos": 0},
                "stdout": "",
                "stderr": "",
            })
            continue

        todo = t.get("todo")
        if todo is not None:
            results.append({
                "test": t,
                "outcome": {"status": "todo", "detail": {"description": todo or None}},
                "duration": {"secs": 0, "nanos": 0},
                "stdout": "",
                "stderr": "",
            })
            continue

        xfail = t.get("xfail")
        is_xfail = xfail is not None

        # Set up soft assertion context (matches real worker)
        ctx = SoftContext()
        _set_soft_context(ctx)
        start = time.monotonic()

        try:
            if case_index is not None and hasattr(fn, "__tryke_cases__"):
                cases = fn.__tryke_cases__
                if case_index < len(cases):
                    entry = cases[case_index]
                    fn(*entry.args, **entry.kwargs)
                else:
                    fn()
            else:
                fn()

            elapsed = time.monotonic() - start
            secs = int(elapsed)
            nanos = int((elapsed - secs) * 1_000_000_000)

            if ctx.failures:
                if is_xfail:
                    results.append({
                        "test": t,
                        "outcome": {"status": "x_failed", "detail": {"reason": xfail or None}},
                        "duration": {"secs": secs, "nanos": nanos},
                        "stdout": "", "stderr": "",
                    })
                else:
                    results.append({
                        "test": t,
                        "outcome": {
                            "status": "failed",
                            "detail": {
                                "message": "assertion failed",
                                "traceback": None,
                                "assertions": _extract_soft_failures(ctx.failures),
                                "executed_lines": list(ctx.executed_lines),
                            }
                        },
                        "duration": {"secs": secs, "nanos": nanos},
                        "stdout": "", "stderr": "",
                    })
            elif is_xfail:
                results.append({
                    "test": t,
                    "outcome": {"status": "x_passed"},
                    "duration": {"secs": secs, "nanos": nanos},
                    "stdout": "", "stderr": "",
                })
            else:
                results.append({
                    "test": t,
                    "outcome": {"status": "passed"},
                    "duration": {"secs": secs, "nanos": nanos},
                    "stdout": "", "stderr": "",
                })

        except ExpectationError as exc:
            elapsed = time.monotonic() - start
            secs = int(elapsed)
            nanos = int((elapsed - secs) * 1_000_000_000)
            if is_xfail:
                results.append({
                    "test": t,
                    "outcome": {"status": "x_failed", "detail": {"reason": xfail or None}},
                    "duration": {"secs": secs, "nanos": nanos},
                    "stdout": "", "stderr": "",
                })
            else:
                assertions = _extract_soft_failures(ctx.failures)
                tb_frames = traceback.extract_tb(sys.exc_info()[2])
                frame = None
                for f in reversed(tb_frames):
                    if "/tryke/" not in f.filename:
                        frame = f
                        break
                if frame:
                    assertions.append({
                        "expression": (frame.line or "").strip(),
                        "file": frame.filename,
                        "line": frame.lineno,
                        "span_offset": getattr(exc, "span_offset", 0),
                        "span_length": getattr(exc, "span_length", 0),
                        "expected": str(getattr(exc, "expected", "")),
                        "received": str(getattr(exc, "received", "")),
                    })
                results.append({
                    "test": t,
                    "outcome": {
                        "status": "failed",
                        "detail": {
                            "message": str(exc) or "assertion failed",
                            "traceback": traceback.format_exc(),
                            "assertions": assertions,
                            "executed_lines": list(ctx.executed_lines),
                        }
                    },
                    "duration": {"secs": secs, "nanos": nanos},
                    "stdout": "", "stderr": "",
                })

        except Exception as exc:
            elapsed = time.monotonic() - start
            secs = int(elapsed)
            nanos = int((elapsed - secs) * 1_000_000_000)
            if is_xfail:
                results.append({
                    "test": t,
                    "outcome": {"status": "x_failed", "detail": {"reason": xfail or None}},
                    "duration": {"secs": secs, "nanos": nanos},
                    "stdout": "", "stderr": "",
                })
            else:
                results.append({
                    "test": t,
                    "outcome": {
                        "status": "failed",
                        "detail": {
                            "message": f"{type(exc).__name__}: {exc}",
                            "traceback": traceback.format_exc(),
                            "assertions": _extract_soft_failures(ctx.failures),
                            "executed_lines": list(ctx.executed_lines),
                        }
                    },
                    "duration": {"secs": secs, "nanos": nanos},
                    "stdout": "", "stderr": "",
                })
        finally:
            _set_soft_context(None)

    return json.dumps(results)
`;

async function init() {
  pyodide = await loadPyodide();

  // Write tryke package to virtual FS
  pyodide.FS.mkdirTree("/home/pyodide/tryke");
  pyodide.FS.writeFile("/home/pyodide/tryke/__init__.py", initSource);
  pyodide.FS.writeFile("/home/pyodide/tryke/expect.py", expectSource);
  pyodide.FS.writeFile("/home/pyodide/tryke/hooks.py", hooksSource);
  pyodide.FS.writeFile("/home/pyodide/tryke_guard.py", guardSource);

  // Add /home/pyodide to sys.path
  pyodide.runPython(`
import sys
if "/home/pyodide" not in sys.path:
    sys.path.insert(0, "/home/pyodide")
`);

  // Load the runner
  pyodide.runPython(RUNNER);

  self.postMessage({ type: "ready" });
}

self.onmessage = async (e: MessageEvent) => {
  const { type } = e.data;

  if (type === "init") {
    try {
      await init();
    } catch (err) {
      self.postMessage({
        type: "error",
        message: `Pyodide init failed: ${err}`,
      });
    }
    return;
  }

  if (type === "run") {
    if (!pyodide) {
      self.postMessage({ type: "error", message: "Pyodide not initialized" });
      return;
    }

    const { filename, source, tests } = e.data;
    try {
      const resultsJson = pyodide.runPython(
        `run_tests(${JSON.stringify(filename)}, ${JSON.stringify(source)}, ${JSON.stringify(JSON.stringify(tests))})`
      );
      self.postMessage({ type: "result", results: resultsJson });
    } catch (err) {
      self.postMessage({
        type: "error",
        message: `Test execution failed: ${err}`,
      });
    }
  }
};
