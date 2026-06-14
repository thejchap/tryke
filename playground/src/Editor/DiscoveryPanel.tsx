import { useMemo, useState } from "react";
import type { DiscoveredFile, TestItem } from "./types";

interface Props {
  discovery: DiscoveredFile | null;
}

interface TreeNode {
  name: string;
  tests: TestItem[];
  children: Map<string, TreeNode>;
}

function buildTree(tests: TestItem[]): TreeNode {
  const root: TreeNode = { name: "", tests: [], children: new Map() };

  for (const t of tests) {
    const groups = t.groups ?? [];
    let node = root;
    for (const group of groups) {
      if (!node.children.has(group)) {
        node.children.set(group, {
          name: group,
          tests: [],
          children: new Map(),
        });
      }
      node = node.children.get(group)!;
    }
    node.tests.push(t);
  }

  return root;
}

function TestRow({ t }: { t: TestItem }) {
  return (
    <li className="flex items-center gap-2">
      <span className="text-green">&#x25cf;</span>
      <span className="text-text">
        {t.display_name ?? t.name}
        {t.case_label ? `[${t.case_label}]` : ""}
      </span>
      {t.line_number != null && (
        <span className="text-text-dim">:{t.line_number}</span>
      )}
      {t.skip != null && (
        <span
          className="text-yellow text-xs px-1 rounded bg-yellow/10"
          title={t.skip || "This test is skipped"}
        >
          skip
        </span>
      )}
      {t.todo != null && (
        <span
          className="text-accent text-xs px-1 rounded bg-accent/10"
          title={t.todo || "This test is not yet implemented"}
        >
          todo
        </span>
      )}
      {t.xfail != null && (
        <span
          className="text-text-dim text-xs px-1 rounded bg-text-dim/10"
          title={t.xfail || "This test is expected to fail"}
        >
          xfail
        </span>
      )}
      {(t.expected_assertions?.length ?? 0) > 0 && (
        <span className="text-text-dim text-xs">
          ({t.expected_assertions?.length ?? 0} assertions)
        </span>
      )}
    </li>
  );
}

function GroupNode({ node, depth }: { node: TreeNode; depth: number }) {
  const [expanded, setExpanded] = useState(true);
  const childCount = countTests(node);

  return (
    <div style={{ marginLeft: depth > 0 ? 12 : 0 }}>
      <button
        onClick={() => setExpanded(!expanded)}
        className="flex items-center gap-1.5 text-text hover:text-accent transition-colors py-0.5"
      >
        <span className="text-text-dim text-[10px]">
          {expanded ? "▼" : "▶"}
        </span>
        <span className="font-medium">{node.name}</span>
        <span className="text-text-dim text-xs">({childCount})</span>
      </button>
      {expanded && (
        <div className="ml-3">
          {node.tests.length > 0 && (
            <ul className="space-y-0.5">
              {node.tests.map((t, i) => (
                <TestRow key={i} t={t} />
              ))}
            </ul>
          )}
          {[...node.children.values()].map((child) => (
            <GroupNode key={child.name} node={child} depth={depth + 1} />
          ))}
        </div>
      )}
    </div>
  );
}

function countTests(node: TreeNode): number {
  let count = node.tests.length;
  for (const child of node.children.values()) {
    count += countTests(child);
  }
  return count;
}

export function DiscoveryPanel({ discovery }: Props) {
  const tree = useMemo(() => {
    if (!discovery) return null;
    return buildTree(discovery.parsed.tests ?? []);
  }, [discovery]);

  if (!discovery) {
    return (
      <div className="p-4 text-text-dim text-sm">
        Write some tests to see discovery results.
      </div>
    );
  }

  const tests = discovery.parsed.tests ?? [];
  const fixtures = discovery.parsed.hooks ?? [];
  const errors = discovery.parsed.errors ?? [];

  return (
    <div className="p-3 text-sm overflow-auto h-full">
      {errors.length > 0 && (
        <div className="mb-3">
          <h3 className="text-red font-bold mb-1">Errors</h3>
          {errors.map((err, i) => (
            <div key={i} className="text-red/80 ml-2">
              {err}
            </div>
          ))}
        </div>
      )}

      <div className="mb-3">
        <h3 className="text-text font-bold mb-1">Tests ({tests.length})</h3>
        {tests.length === 0 ? (
          <div className="text-text-dim ml-2">No tests found.</div>
        ) : tree && tree.children.size > 0 ? (
          <div className="ml-1">
            {tree.tests.length > 0 && (
              <ul className="space-y-0.5 ml-2">
                {tree.tests.map((t, i) => (
                  <TestRow key={i} t={t} />
                ))}
              </ul>
            )}
            {[...tree.children.values()].map((child) => (
              <GroupNode key={child.name} node={child} depth={0} />
            ))}
          </div>
        ) : (
          <ul className="ml-2 space-y-0.5">
            {tests.map((t, i) => (
              <TestRow key={i} t={t} />
            ))}
          </ul>
        )}
      </div>

      {fixtures.length > 0 && (
        <div className="mb-3">
          <h3 className="text-text font-bold mb-1">
            Fixtures ({fixtures.length})
          </h3>
          <ul className="ml-2 space-y-0.5">
            {fixtures.map((h, i) => (
              <li key={i} className="flex items-center gap-2">
                <span className="text-accent">&#x25cf;</span>
                <span className="text-text">{h.name}</span>
                <span
                  className="text-text-dim text-xs"
                  title={
                    h.per === "test"
                      ? "Re-created for each test"
                      : "Created once, shared across all tests"
                  }
                >
                  per:{h.per}
                </span>
                {(h.depends_on?.length ?? 0) > 0 && (
                  <span className="text-text-dim text-xs">
                    deps: {h.depends_on?.join(", ")}
                  </span>
                )}
              </li>
            ))}
          </ul>
        </div>
      )}

      {discovery.dynamic_imports && (
        <div className="text-yellow text-xs mt-2">
          Dynamic imports detected — this file will always re-run with
          --changed.
        </div>
      )}
    </div>
  );
}
