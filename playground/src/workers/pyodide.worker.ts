/// <reference lib="webworker" />

import initSource from "../../../python/tryke/__init__.py?raw";
import expectSource from "../../../python/tryke/expect.py?raw";
import hooksSource from "../../../python/tryke/hooks.py?raw";
import runnerSource from "../../../python/tryke/runner.py?raw";
import playgroundSource from "../../../python/tryke/playground.py?raw";
import guardSource from "../../../python/tryke_guard.py?raw";

const PYODIDE_CDN = "https://cdn.jsdelivr.net/pyodide/v0.27.6/full/pyodide.mjs";

// Use a dynamic import so Rollup doesn't convert the CDN URL into an
// unresolvable global variable reference in the production bundle.
interface PyodideInterface {
  FS: {
    mkdirTree(path: string): void;
    writeFile(path: string, data: string): void;
  };
  runPython(code: string): string;
}

let pyodide: PyodideInterface | null = null;

async function init() {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const { loadPyodide } = (await import(/* @vite-ignore */ PYODIDE_CDN)) as any;
  const py: PyodideInterface = await loadPyodide();

  // Write tryke package to virtual FS
  py.FS.mkdirTree("/home/pyodide/tryke");
  py.FS.writeFile("/home/pyodide/tryke/__init__.py", initSource);
  py.FS.writeFile("/home/pyodide/tryke/expect.py", expectSource);
  py.FS.writeFile("/home/pyodide/tryke/hooks.py", hooksSource);
  py.FS.writeFile("/home/pyodide/tryke/runner.py", runnerSource);
  py.FS.writeFile("/home/pyodide/tryke/playground.py", playgroundSource);
  py.FS.writeFile("/home/pyodide/tryke_guard.py", guardSource);

  py.runPython(`
import sys
if "/home/pyodide" not in sys.path:
    sys.path.insert(0, "/home/pyodide")

# Flip the guard so code inside "if __TRYKE_TESTING__:" blocks executes
# when user modules are imported — matches what the native worker does.
import tryke_guard
tryke_guard.__TRYKE_TESTING__ = True

from tryke.playground import run_tests
`);

  pyodide = py;
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

    const { runId, filename, source, tests, hooks, allFiles } = e.data;
    try {
      const allFilesArg = allFiles
        ? JSON.stringify(JSON.stringify(allFiles))
        : "None";
      const hooksArg = hooks ? JSON.stringify(JSON.stringify(hooks)) : "None";
      const resultsJson = pyodide.runPython(
        `run_tests(${JSON.stringify(filename)}, ${JSON.stringify(source)}, ${JSON.stringify(JSON.stringify(tests))}, ${allFilesArg}, ${hooksArg})`,
      );
      self.postMessage({ type: "result", runId, results: resultsJson });
    } catch (err) {
      self.postMessage({
        type: "error",
        runId,
        message: `Test execution failed: ${err}`,
      });
    }
  }
};
