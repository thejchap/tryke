import type { SecondaryTool } from "./types";

interface Props {
  active: SecondaryTool;
  onChange: (tool: SecondaryTool) => void;
  hasOutput: boolean;
}

const TABS: { id: SecondaryTool; label: string }[] = [
  { id: "discovery", label: "Discovery" },
  { id: "import-graph", label: "Import Graph" },
  { id: "fixture-graph", label: "Fixture Graph" },
  { id: "output", label: "Output" },
];

export function SecondarySideBar({ active, onChange, hasOutput }: Props) {
  const isAll = active === "all";

  return (
    <div className="flex border-b border-border">
      {TABS.map((tab) => (
        <button
          key={tab.id}
          onClick={() => onChange(tab.id)}
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
        title={isAll ? "Show single panel" : "Show all panels"}
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
