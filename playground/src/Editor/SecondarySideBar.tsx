import type { SecondaryTool } from "./types";

interface Props {
  active: SecondaryTool;
  onChange: (tool: SecondaryTool) => void;
  hasOutput: boolean;
}

const TABS: { id: SecondaryTool; label: string }[] = [
  { id: "discovery", label: "Discovery" },
  { id: "graph", label: "Graph" },
  { id: "output", label: "Output" },
];

export function SecondarySideBar({ active, onChange, hasOutput }: Props) {
  return (
    <div className="flex border-b border-border">
      {TABS.map((tab) => (
        <button
          key={tab.id}
          onClick={() => onChange(tab.id)}
          className={`px-3 py-1.5 text-xs font-medium transition-colors ${
            active === tab.id
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
    </div>
  );
}
