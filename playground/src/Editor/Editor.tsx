import { useDeferredValue, useMemo } from "react";
import { Panel, PanelGroup, PanelResizeHandle } from "react-resizable-panels";

import type {
  DiscoveredFile,
  GraphEdge,
  PlaygroundFile,
  ReporterName,
  RunStatus,
  SecondaryTool,
} from "./types";
import { SourceEditor } from "./SourceEditor";
import { DiscoveryPanel } from "./DiscoveryPanel";
import { GraphView } from "./GraphView";
import { TerminalOutput } from "./TerminalOutput";
import { SecondarySideBar } from "./SecondarySideBar";

interface WasmModule {
  discover: (source: string, filename: string) => DiscoveredFile;
  discover_multi: (files_json: string) => {
    files: { path: string; discovered: DiscoveredFile }[];
    edges: GraphEdge[];
  };
  format_results: (results_json: string, reporter: string) => string;
  format_collect: (tests_json: string, reporter: string) => string;
}

interface Props {
  files: PlaygroundFile[];
  activeFileIndex: number;
  onSourceChange: (source: string) => void;
  secondaryTool: SecondaryTool;
  onSecondaryToolChange: (tool: SecondaryTool) => void;
  reporter: ReporterName;
  terminalOutput: string;
  runStatus: RunStatus;
  wasm: WasmModule | null;
}

export function Editor({
  files,
  activeFileIndex,
  onSourceChange,
  secondaryTool,
  onSecondaryToolChange,
  reporter,
  terminalOutput,
  runStatus,
  wasm,
}: Props) {
  const activeFile = files[activeFileIndex]!;
  const deferredSource = useDeferredValue(activeFile.source);

  const discovery = useMemo<DiscoveredFile | null>(() => {
    if (!wasm) return null;
    try {
      return wasm.discover(deferredSource, activeFile.name);
    } catch {
      return null;
    }
  }, [wasm, deferredSource, activeFile.name]);

  const multiDiscovery = useMemo(() => {
    if (!wasm || files.length < 2)
      return { edges: [] as GraphEdge[], files: files.map((f) => f.name) };
    try {
      const result = wasm.discover_multi(
        JSON.stringify(
          files.map((f) => ({ filename: f.name, source: f.source })),
        ),
      );
      return {
        edges: result.edges,
        files: result.files.map((f) => f.path),
      };
    } catch {
      return { edges: [] as GraphEdge[], files: files.map((f) => f.name) };
    }
  }, [wasm, files]);

  const collectOutput = useMemo(() => {
    if (!wasm || !discovery) return "";
    try {
      const tests = JSON.stringify(discovery.parsed.tests);
      return wasm.format_collect(tests, reporter);
    } catch {
      return "";
    }
  }, [wasm, discovery, reporter]);

  const displayOutput = runStatus !== "idle" ? terminalOutput : collectOutput;

  return (
    <PanelGroup direction="horizontal" className="h-full">
      <Panel defaultSize={55} minSize={30}>
        <SourceEditor
          source={activeFile.source}
          filename={activeFile.name}
          onChange={onSourceChange}
        />
      </Panel>
      <PanelResizeHandle className="w-1 bg-border hover:bg-accent transition-colors" />
      <Panel defaultSize={45} minSize={25}>
        <div className="h-full flex flex-col bg-surface">
          <SecondarySideBar
            active={secondaryTool}
            onChange={onSecondaryToolChange}
            hasOutput={terminalOutput.length > 0}
          />
          <div className="flex-1 overflow-hidden">
            {secondaryTool === "discovery" && (
              <DiscoveryPanel discovery={discovery} />
            )}
            {secondaryTool === "graph" && (
              <GraphView
                edges={multiDiscovery.edges}
                files={multiDiscovery.files}
              />
            )}
            {secondaryTool === "output" && (
              <TerminalOutput content={displayOutput} />
            )}
          </div>
        </div>
      </Panel>
    </PanelGroup>
  );
}
