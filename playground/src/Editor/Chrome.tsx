import { useCallback, useEffect, useRef, useState } from "react";
import type {
  DiscoveredFile,
  GraphEdge,
  HookItem,
  PlaygroundFile,
  ReporterName,
  RunStatus,
  SecondaryTool,
  TestItem,
} from "./types";
import { EXAMPLES, KITCHEN_SINK } from "./constants";
import { Editor } from "./Editor";

interface WasmModule {
  discover: (source: string, filename: string) => DiscoveredFile;
  discover_multi: (files_json: string) => {
    files: { path: string; discovered: DiscoveredFile }[];
    edges: GraphEdge[];
  };
  format_results: (results_json: string, reporter: string) => string;
  format_collect: (tests_json: string, reporter: string) => string;
  version: () => string;
}

interface RunRequest {
  type: "run";
  runId: number;
  filename: string;
  source: string;
  tests: TestItem[];
  hooks: HookItem[];
  allFiles: PlaygroundFile[];
}

const RUN_DEBOUNCE_MS = 500;

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function Chrome() {
  const [files, setFiles] = useState<PlaygroundFile[]>(KITCHEN_SINK.files);
  const [activeFileIndex, setActiveFileIndex] = useState(0);
  const [secondaryTool, setSecondaryTool] = useState<SecondaryTool>("all");
  const [reporter, setReporter] = useState<ReporterName>("text");
  const [pyodideReady, setPyodideReady] = useState(true);
  const [terminalOutput, setTerminalOutput] = useState("");
  const [runStatus, setRunStatus] = useState<RunStatus>("idle");
  const [wasm, setWasm] = useState<WasmModule | null>(null);
  const [wasmVersion, setWasmVersion] = useState("");
  const [newFileName, setNewFileName] = useState("");
  const [showNewFile, setShowNewFile] = useState(false);

  const workerRef = useRef<Worker | null>(null);
  const lastResultsRef = useRef<string>("");
  const wasmRef = useRef<WasmModule | null>(null);
  const reporterRef = useRef<ReporterName>(reporter);
  const runTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const hasAutoRun = useRef(false);
  const runIdRef = useRef(0);

  useEffect(() => {
    wasmRef.current = wasm;
  }, [wasm]);
  useEffect(() => {
    reporterRef.current = reporter;
  }, [reporter]);

  const invalidateRunState = useCallback(() => {
    runIdRef.current += 1;
    lastResultsRef.current = "";
    workerRef.current?.terminate();
    workerRef.current = null;
    setPyodideReady(true);
  }, []);

  // Init WASM
  useEffect(() => {
    (async () => {
      const mod = await import("../wasm/pkg/tryke_wasm.js");
      await mod.default();
      setWasm(mod as unknown as WasmModule);
      setWasmVersion(mod.version());
    })();
  }, []);

  const startIsolatedRun = useCallback((request: RunRequest) => {
    workerRef.current?.terminate();
    setPyodideReady(false);

    const worker = new Worker(
      new URL("../workers/pyodide.worker.ts", import.meta.url),
      { type: "module" },
    );
    workerRef.current = worker;

    const finishWorker = () => {
      worker.terminate();
      if (workerRef.current === worker) {
        workerRef.current = null;
        setPyodideReady(true);
      }
    };

    worker.onmessage = (e: MessageEvent) => {
      if (workerRef.current !== worker) return;

      const { type } = e.data;

      if (type === "ready") {
        worker.postMessage(request);
        return;
      }

      if (type === "result") {
        const isCurrentRun = e.data.runId === runIdRef.current;
        finishWorker();
        if (!isCurrentRun) return;

        const resultsJson: string = e.data.results;
        lastResultsRef.current = resultsJson;
        const w = wasmRef.current;
        if (w) {
          try {
            const output = w.format_results(resultsJson, reporterRef.current);
            setTerminalOutput(output);
          } catch (error) {
            setTerminalOutput(
              `Error formatting ${reporterRef.current} output: ${errorMessage(error)}`,
            );
          }
        } else {
          setTerminalOutput(resultsJson);
        }
        setRunStatus("done");
        return;
      }

      if (type === "error") {
        const isCurrentRun =
          e.data.runId === undefined
            ? request.runId === runIdRef.current
            : e.data.runId === runIdRef.current;
        finishWorker();
        if (!isCurrentRun) return;

        setTerminalOutput(`Error: ${e.data.message}`);
        setRunStatus("done");
      }
    };

    worker.onerror = (event: ErrorEvent) => {
      if (workerRef.current !== worker) return;

      const isCurrentRun = request.runId === runIdRef.current;
      finishWorker();
      if (!isCurrentRun) return;

      setTerminalOutput(`Error: Pyodide worker failed: ${event.message}`);
      setRunStatus("done");
    };

    worker.postMessage({ type: "init" });
  }, []);

  useEffect(() => {
    return () => workerRef.current?.terminate();
  }, []);

  // Re-render when reporter changes and we have results
  useEffect(() => {
    if (!wasm || !lastResultsRef.current || runStatus === "idle") return;
    try {
      const output = wasm.format_results(lastResultsRef.current, reporter);
      setTerminalOutput(output);
    } catch (error) {
      setTerminalOutput(
        `Error formatting ${reporter} output: ${errorMessage(error)}`,
      );
    }
  }, [reporter, wasm, runStatus]);

  const handleRun = useCallback(() => {
    if (!wasm) return;

    const activeFile = files[activeFileIndex]!;

    let tests: TestItem[];
    let hooks: HookItem[];
    try {
      const discovery = wasm.discover(activeFile.source, activeFile.name);
      tests = discovery.parsed.tests;
      hooks = discovery.parsed.hooks;
    } catch {
      invalidateRunState();
      setTerminalOutput("Discovery failed — check your Python syntax.");
      setRunStatus("done");
      return;
    }

    if (tests.length === 0) {
      invalidateRunState();
      setTerminalOutput("No tests discovered in the current file.");
      setRunStatus("done");
      return;
    }

    setRunStatus("running");
    setTerminalOutput("");

    const runId = ++runIdRef.current;
    startIsolatedRun({
      type: "run",
      runId,
      filename: activeFile.name,
      source: activeFile.source,
      tests,
      hooks,
      allFiles: files.map((f) => ({ name: f.name, source: f.source })),
    });
  }, [wasm, files, activeFileIndex, invalidateRunState, startIsolatedRun]);

  // Run on initial WASM load and re-run on source change (debounced).
  useEffect(() => {
    if (!wasm) return;
    if (!hasAutoRun.current) {
      hasAutoRun.current = true;
      handleRun();
      return;
    }

    if (runTimerRef.current) clearTimeout(runTimerRef.current);
    runTimerRef.current = setTimeout(() => {
      handleRun();
    }, RUN_DEBOUNCE_MS);
    return () => {
      if (runTimerRef.current) clearTimeout(runTimerRef.current);
    };
  }, [files, activeFileIndex, wasm, handleRun]);

  // Cmd+Enter / Ctrl+Enter shortcut to run tests
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
        e.preventDefault();
        handleRun();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [handleRun]);

  const handleSourceChange = useCallback(
    (source: string) => {
      invalidateRunState();
      setFiles((prev) =>
        prev.map((f, i) => (i === activeFileIndex ? { ...f, source } : f)),
      );
    },
    [activeFileIndex, invalidateRunState],
  );

  const handleAddFile = useCallback(() => {
    if (!newFileName) return;
    invalidateRunState();
    const name = newFileName.endsWith(".py")
      ? newFileName
      : `${newFileName}.py`;
    const existingIndex = files.findIndex((f) => f.name === name);
    if (existingIndex !== -1) {
      setActiveFileIndex(existingIndex);
    } else {
      setFiles((prev) => [...prev, { name, source: "" }]);
      setActiveFileIndex(files.length);
    }
    setNewFileName("");
    setShowNewFile(false);
  }, [newFileName, files, invalidateRunState]);

  const handleRemoveFile = useCallback(
    (index: number) => {
      if (files.length <= 1) return;
      invalidateRunState();
      setFiles((prev) => prev.filter((_, i) => i !== index));
      setActiveFileIndex((prev) =>
        prev >= index ? Math.max(0, prev - 1) : prev,
      );
    },
    [files.length, invalidateRunState],
  );

  const handleLoadExample = useCallback(
    (exampleIndex: number) => {
      const example = EXAMPLES[exampleIndex];
      if (!example) return;
      invalidateRunState();
      setFiles(example.files);
      setActiveFileIndex(0);
      setRunStatus("idle");
      setTerminalOutput("");
    },
    [invalidateRunState],
  );

  return (
    <div className="h-full flex flex-col bg-bg text-text">
      {/* Toolbar */}
      <div className="flex items-center gap-3 px-4 py-2 border-b border-border bg-surface">
        <a href="/" className="flex items-center gap-2 hover:opacity-80">
          <img src="/logo.png" alt="tryke" className="h-6 w-6 rounded" />
          <h1 className="text-sm font-bold text-white">Tryke Playground</h1>
        </a>
        {wasmVersion && (
          <span className="text-xs text-text-dim">v{wasmVersion}</span>
        )}

        <a
          href="https://tryke.dev"
          target="_blank"
          rel="noopener noreferrer"
          className="text-xs text-text-dim hover:text-accent transition-colors"
          title="Open tryke documentation"
        >
          Docs
        </a>

        <div className="flex-1" />

        {/* Example picker */}
        <select
          className="text-xs bg-bg border border-border rounded px-2 py-1 text-text"
          value=""
          onChange={(e) => handleLoadExample(Number(e.target.value))}
          title="Load a pre-built example"
        >
          <option value="" disabled>
            Examples
          </option>
          {EXAMPLES.map((ex, i) => (
            <option key={i} value={i}>
              {ex.label}
            </option>
          ))}
        </select>

        {/* Reporter picker */}
        <select
          className="text-xs bg-bg border border-border rounded px-2 py-1 text-text"
          value={reporter}
          onChange={(e) => setReporter(e.target.value as ReporterName)}
          title="Output format (same as --reporter CLI flag)"
        >
          <option value="text">text</option>
          <option value="dot">dot</option>
          <option value="next">next</option>
          <option value="sugar">sugar</option>
          <option value="json">json</option>
          <option value="llm">llm</option>
        </select>

        {/* Pyodide status */}
        <span
          className={`text-xs px-2 py-0.5 rounded ${
            pyodideReady ? "bg-green/10 text-green" : "bg-yellow/10 text-yellow"
          }`}
          title={
            pyodideReady
              ? "Each run starts in a fresh Pyodide worker so import-time side effects are discarded"
              : "Loading an isolated Pyodide worker for this run"
          }
        >
          {pyodideReady ? "Python ready" : "Loading Python..."}
        </span>

        {/* Run button */}
        <button
          onClick={handleRun}
          disabled={!wasm || runStatus === "running"}
          className="text-xs font-bold px-3 py-1 rounded bg-green/20 text-green hover:bg-green/30 disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
          title="Run tests in the active file (⌘Enter)"
        >
          {runStatus === "running" ? "Running..." : "Run ⌘⏎"}
        </button>
      </div>

      {/* File tabs */}
      <div
        className="flex items-center border-b border-border bg-surface"
        role="tablist"
      >
        {files.map((file, i) => (
          <div
            key={file.name}
            role="tab"
            tabIndex={0}
            aria-selected={i === activeFileIndex}
            className={`flex items-center gap-1 px-3 py-1.5 text-xs cursor-pointer border-r border-border ${
              i === activeFileIndex
                ? "bg-bg text-text"
                : "text-text-dim hover:text-text hover:bg-surface-hover"
            }`}
            onClick={() => setActiveFileIndex(i)}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                setActiveFileIndex(i);
              }
            }}
          >
            <span>{file.name}</span>
            {files.length > 1 && (
              <button
                onClick={(e) => {
                  e.stopPropagation();
                  handleRemoveFile(i);
                }}
                className="ml-1 text-text-dim hover:text-red text-[10px]"
                title="Remove file"
              >
                x
              </button>
            )}
          </div>
        ))}
        {showNewFile ? (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              handleAddFile();
            }}
            className="flex items-center px-2"
          >
            <input
              autoFocus
              className="text-xs bg-bg border border-border rounded px-2 py-0.5 text-text w-32"
              placeholder="filename.py"
              value={newFileName}
              onChange={(e) => setNewFileName(e.target.value)}
              onBlur={() => setShowNewFile(false)}
            />
          </form>
        ) : (
          <button
            onClick={() => setShowNewFile(true)}
            className="px-2 py-1.5 text-xs text-text-dim hover:text-text"
            title="Add a new file"
          >
            +
          </button>
        )}
      </div>

      {/* Editor area */}
      <div className="flex-1 overflow-hidden">
        <Editor
          files={files}
          activeFileIndex={activeFileIndex}
          onSourceChange={handleSourceChange}
          secondaryTool={secondaryTool}
          onSecondaryToolChange={setSecondaryTool}
          reporter={reporter}
          terminalOutput={terminalOutput}
          runStatus={runStatus}
          wasm={wasm}
          pyodideReady={pyodideReady}
        />
      </div>
    </div>
  );
}
