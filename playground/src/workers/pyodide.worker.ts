/// <reference lib="webworker" />

// prettier-ignore
// @ts-expect-error pyodide loaded from CDN
import { loadPyodide, type PyodideInterface } from "https://cdn.jsdelivr.net/pyodide/v0.27.6/full/pyodide.mjs";

import initSource from "../../../python/tryke/__init__.py?raw";
import expectSource from "../../../python/tryke/expect.py?raw";
import hooksSource from "../../../python/tryke/hooks.py?raw";
import runnerSource from "../../../python/tryke/runner.py?raw";
import playgroundSource from "../../../python/tryke/playground.py?raw";
import guardSource from "../../../python/tryke_guard.py?raw";

let pyodide: PyodideInterface | null = null;

async function init() {
  pyodide = await loadPyodide();

  // Write tryke package to virtual FS
  pyodide.FS.mkdirTree("/home/pyodide/tryke");
  pyodide.FS.writeFile("/home/pyodide/tryke/__init__.py", initSource);
  pyodide.FS.writeFile("/home/pyodide/tryke/expect.py", expectSource);
  pyodide.FS.writeFile("/home/pyodide/tryke/hooks.py", hooksSource);
  pyodide.FS.writeFile("/home/pyodide/tryke/runner.py", runnerSource);
  pyodide.FS.writeFile("/home/pyodide/tryke/playground.py", playgroundSource);
  pyodide.FS.writeFile("/home/pyodide/tryke_guard.py", guardSource);

  pyodide.runPython(`
import sys
if "/home/pyodide" not in sys.path:
    sys.path.insert(0, "/home/pyodide")
from tryke.playground import run_tests
`);

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

    const { filename, source, tests, allFiles } = e.data;
    try {
      const allFilesArg = allFiles
        ? JSON.stringify(JSON.stringify(allFiles))
        : "None";
      const resultsJson = pyodide.runPython(
        `run_tests(${JSON.stringify(filename)}, ${JSON.stringify(source)}, ${JSON.stringify(JSON.stringify(tests))}, ${allFilesArg})`,
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
