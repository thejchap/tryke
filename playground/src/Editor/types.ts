export interface PlaygroundFile {
  name: string;
  source: string;
}

export interface ExpectedAssertion {
  subject: string;
  matcher: string;
  negated: boolean;
  args: string[];
  line: number;
  label: string | null;
}

export interface TestItem {
  name: string;
  module_path: string;
  file_path: string | null;
  line_number: number | null;
  display_name: string | null;
  expected_assertions: ExpectedAssertion[];
  skip: string | null;
  todo: string | null;
  xfail: string | null;
  tags: string[];
  groups: string[];
  case_label: string | null;
  case_index: number | null;
}

export interface HookItem {
  name: string;
  module_path: string;
  per: "test" | "scope";
  groups: string[];
  depends_on: string[];
  line_number: number | null;
}

export interface ParsedFile {
  tests: TestItem[];
  hooks: HookItem[];
  testing_guard_else_lines: number[];
  errors: string[];
}

export interface DiscoveredFile {
  parsed: ParsedFile;
  import_candidates: string[][];
  dynamic_imports: boolean;
}

export interface GraphEdge {
  from: string;
  to: string;
}

export interface FileResult {
  path: string;
  discovered: DiscoveredFile;
}

export interface MultiResult {
  files: FileResult[];
  edges: GraphEdge[];
}

export type ReporterName = "text" | "dot" | "json" | "llm";
export type SecondaryTool =
  | "discovery"
  | "import-graph"
  | "fixture-graph"
  | "output"
  | "all";
export type RunStatus = "idle" | "running" | "done";
