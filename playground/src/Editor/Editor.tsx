import { type ReactNode, useDeferredValue, useMemo } from "react";
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
import { FixtureGraphView } from "./FixtureGraphView";
import { TerminalOutput } from "./TerminalOutput";
import { SecondarySideBar } from "./SecondarySideBar";

function Section({
  title,
  children,
  fill,
}: {
  title: string;
  children: ReactNode;
  fill?: boolean;
}) {
  return (
    <div className={fill ? "h-full flex flex-col" : "border-b border-border"}>
      <div className="shrink-0 px-3 py-1.5 text-xs font-bold text-text-dim bg-bg/50 border-b border-border">
        {title}
      </div>
      <div className={fill ? "flex-1 min-h-0" : undefined}>{children}</div>
    </div>
  );
}

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
            {secondaryTool === "all" ? (
              <div className="h-full flex flex-col">
                <div className="shrink-0 overflow-auto max-h-[40%] border-b border-border">
                  <Section title="Discovery">
                    <DiscoveryPanel discovery={discovery} />
                  </Section>
                </div>
                <div className="shrink-0 overflow-auto max-h-[30%] border-b border-border">
                  <Section title="Import Graph">
                    <GraphView
                      edges={multiDiscovery.edges}
                      files={multiDiscovery.files}
                    />
                  </Section>
                  <Section title="Fixture Graph">
                    <FixtureGraphView hooks={discovery?.parsed.hooks ?? []} />
                  </Section>
                </div>
                <div className="flex-1 min-h-0">
                  <Section title="Output" fill>
                    <TerminalOutput content={displayOutput} />
                  </Section>
                </div>
              </div>
            ) : (
              <>
                {secondaryTool === "discovery" && (
                  <DiscoveryPanel discovery={discovery} />
                )}
                {secondaryTool === "import-graph" && (
                  <GraphView
                    edges={multiDiscovery.edges}
                    files={multiDiscovery.files}
                  />
                )}
                {secondaryTool === "fixture-graph" && (
                  <FixtureGraphView hooks={discovery?.parsed.hooks ?? []} />
                )}
                {secondaryTool === "output" && (
                  <TerminalOutput content={displayOutput} />
                )}
              </>
            )}
          </div>
        </div>
      </Panel>
    </PanelGroup>
  );
}
