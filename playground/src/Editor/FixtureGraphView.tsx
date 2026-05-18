import { useMemo } from "react";
import dagre from "@dagrejs/dagre";
import type { HookItem } from "./types";

interface Props {
  hooks: HookItem[];
}

interface FixtureEdge {
  from: string;
  to: string;
}

const NODE_WIDTH = 140;
const NODE_HEIGHT = 48;

export function FixtureGraphView({ hooks }: Props) {
  const scopeMap = useMemo(() => {
    const map = new Map<string, string>();
    for (const h of hooks) {
      map.set(h.name, h.per);
    }
    return map;
  }, [hooks]);

  const { nodes: fixtureNames, edges } = useMemo(() => {
    const names = new Set<string>();
    const edgeList: FixtureEdge[] = [];

    for (const h of hooks) {
      names.add(h.name);
      for (const dep of h.depends_on ?? []) {
        names.add(dep);
        edgeList.push({ from: dep, to: h.name });
      }
    }

    return { nodes: [...names], edges: edgeList };
  }, [hooks]);

  const layout = useMemo(() => {
    if (fixtureNames.length === 0) return null;

    const g = new dagre.graphlib.Graph();
    g.setGraph({ rankdir: "TB", nodesep: 40, ranksep: 60 });
    g.setDefaultEdgeLabel(() => ({}));

    for (const name of fixtureNames) {
      g.setNode(name, { width: NODE_WIDTH, height: NODE_HEIGHT });
    }
    for (const e of edges) {
      g.setEdge(e.from, e.to);
    }

    dagre.layout(g);

    const nodes = g.nodes().map((id) => {
      const node = g.node(id);
      return {
        id,
        x: node.x,
        y: node.y,
        width: node.width,
        height: node.height,
      };
    });

    const edgeLines = g.edges().map((e) => {
      const edge = g.edge(e);
      return { points: edge.points, from: e.v, to: e.w };
    });

    const graph = g.graph();
    const width = (graph.width ?? 300) + 40;
    const height = (graph.height ?? 200) + 40;

    return { nodes, edges: edgeLines, width, height };
  }, [fixtureNames, edges]);

  if (fixtureNames.length === 0) {
    return (
      <div className="p-4 text-text-dim text-sm">
        No fixtures found. Use <code>@fixture</code> and <code>Depends()</code>{" "}
        to see the dependency graph.
      </div>
    );
  }

  if (!layout) return null;

  return (
    <div className="h-full overflow-auto p-2">
      <svg width={layout.width} height={layout.height} className="mx-auto">
        <defs>
          <marker
            id="fixture-arrowhead"
            markerWidth="10"
            markerHeight="7"
            refX="10"
            refY="3.5"
            orient="auto"
          >
            <polygon points="0 0, 10 3.5, 0 7" fill="#94e2d5" />
          </marker>
        </defs>

        {layout.edges.map((e, i) => {
          const points = e.points.map((p) => `${p.x},${p.y}`).join(" ");
          return (
            <polyline
              key={i}
              points={points}
              fill="none"
              stroke="#94e2d5"
              strokeWidth={1.5}
              markerEnd="url(#fixture-arrowhead)"
            />
          );
        })}

        {layout.nodes.map((n) => {
          const scope = scopeMap.get(n.id);
          const isScope = scope === "scope";
          return (
            <g key={n.id}>
              <rect
                x={n.x - n.width / 2}
                y={n.y - n.height / 2}
                width={n.width}
                height={n.height}
                rx={6}
                fill="#313244"
                stroke={isScope ? "#f9e2af" : "#94e2d5"}
                strokeWidth={1}
                strokeDasharray={isScope ? "4 2" : undefined}
              />
              <text
                x={n.x}
                y={n.y - 2}
                textAnchor="middle"
                fill="#cdd6f4"
                fontSize={12}
                fontFamily="monospace"
              >
                {n.id}
              </text>
              {scope && (
                <text
                  x={n.x}
                  y={n.y + 14}
                  textAnchor="middle"
                  fill={isScope ? "#f9e2af" : "#6c7086"}
                  fontSize={9}
                  fontFamily="monospace"
                >
                  per:{scope}
                </text>
              )}
            </g>
          );
        })}
      </svg>
    </div>
  );
}
