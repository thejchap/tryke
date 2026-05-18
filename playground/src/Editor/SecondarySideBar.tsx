import type { SecondaryTool } from "./types";

interface Props {
  active: SecondaryTool;
  onChange: (tool: SecondaryTool) => void;
  hasOutput: boolean;
}

const TABS: { id: SecondaryTool; label: string; tooltip: string }[] = [
  {
    id: "discovery",
    label: "Discovery",
    tooltip: "Discovered tests and fixtures from static analysis",
  },
  {
    id: "import-graph",
    label: "Import Graph",
    tooltip: "Dependency graph between files",
  },
  {
    id: "fixture-graph",
    label: "Fixture Graph",
    tooltip: "Fixture dependency graph (Depends() relationships)",
  },
  {
    id: "output",
    label: "Output",
    tooltip: "Test runner output",
  },
];

export function SecondarySideBar({ active, onChange, hasOutput }: Props) {
  const isAll = active === "all";

  return (
    <div className="flex border-b border-border">
      {TABS.map((tab) => (
        <button
          key={tab.id}
          onClick={() => onChange(tab.id)}
          title={tab.tooltip}
          className={`px-3 py-1.5 text-xs font-medium transition-colors ${
            !isAll && active === tab.id
              ? "text-accent border-b-2 border-accent"
              : "text-text-dim hover:text-text"
          }`}
        >
          {tab.label}
          {tab.id === "output" && hasOutput && (
            <span className="ml-1 w-1.5 h-1.5 rounded-full bg-green inline-block" />
          )}
        </button>
      ))}
      <div className="flex-1" />
      <button
        onClick={() => onChange(isAll ? "output" : "all")}
        title={isAll ? "Show single panel" : "Show all panels stacked"}
        className={`px-2 py-1.5 text-xs font-medium transition-colors ${
          isAll
            ? "text-accent border-b-2 border-accent"
            : "text-text-dim hover:text-text"
        }`}
      >
        ⊞
      </button>
    </div>
  );
}
