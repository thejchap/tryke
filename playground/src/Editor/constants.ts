import type { PlaygroundFile } from "./types";

import testMathSource from "../../examples/test_math.py?raw";
import testBasicsSource from "../../examples/test_basics.py?raw";
import testSoftSource from "../../examples/test_soft.py?raw";
import testCasesSource from "../../examples/test_cases.py?raw";
import testDescribeSource from "../../examples/test_describe.py?raw";
import helpersSource from "../../examples/helpers.py?raw";
import testHelpersSource from "../../examples/test_helpers.py?raw";
import testFixturesSource from "../../examples/test_fixtures.py?raw";
import testKitchenSinkSource from "../../examples/test_kitchen_sink.py?raw";
import mathlibSource from "../../examples/mathlib.py?raw";
import testMarkersSource from "../../examples/test_markers.py?raw";

export const DEFAULT_FILES: PlaygroundFile[] = [
  { name: "test_math.py", source: testMathSource },
];

export interface Example {
  label: string;
  files: PlaygroundFile[];
}

// The default playground content. Exported by name so callers don't depend
// on its position in EXAMPLES (a reorder there shouldn't change the default).
export const KITCHEN_SINK: Example = {
  label: "Kitchen Sink",
  files: [
    { name: "test_kitchen_sink.py", source: testKitchenSinkSource },
    { name: "mathlib.py", source: mathlibSource },
  ],
};

export const EXAMPLES: Example[] = [
  {
    label: "Basic assertions",
    files: [{ name: "test_basics.py", source: testBasicsSource }],
  },
  {
    label: "Soft assertions",
    files: [{ name: "test_soft.py", source: testSoftSource }],
  },
  {
    label: "Parametrized cases",
    files: [{ name: "test_cases.py", source: testCasesSource }],
  },
  {
    label: "Describe blocks",
    files: [{ name: "test_describe.py", source: testDescribeSource }],
  },
  {
    label: "Multi-file imports",
    files: [
      { name: "test_helpers.py", source: testHelpersSource },
      { name: "helpers.py", source: helpersSource },
    ],
  },
  {
    label: "Fixtures & Depends",
    files: [{ name: "test_fixtures.py", source: testFixturesSource }],
  },
  KITCHEN_SINK,
  {
    label: "Skip / Todo / XFail",
    files: [{ name: "test_markers.py", source: testMarkersSource }],
  },
];
