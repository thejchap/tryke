import { useMemo } from "react";
import dagre from "@dagrejs/dagre";
import type { GraphEdge } from "./types";

interface Props {
  edges: GraphEdge[];
  files: string[];
}

const NODE_WIDTH = 140;
const NODE_HEIGHT = 36;

export function GraphView({ edges, files }: Props) {
  const layout = useMemo(() => {
    if (files.length === 0) return null;

    const g = new dagre.graphlib.Graph();
    g.setGraph({ rankdir: "TB", nodesep: 40, ranksep: 60 });
    g.setDefaultEdgeLabel(() => ({}));

    for (const f of files) {
      g.setNode(f, { width: NODE_WIDTH, height: NODE_HEIGHT });
    }
    for (const e of edges) {
      g.setEdge(e.from, e.to);
    }

    dagre.layout(g);

    const nodes = g.nodes().map((id) => {
      const node = g.node(id);
      return { id, x: node.x, y: node.y, width: node.width, height: node.height };
    });

    const edgeLines = g.edges().map((e) => {
      const edge = g.edge(e);
      return { points: edge.points, from: e.v, to: e.w };
    });

    const graph = g.graph();
    const width = (graph.width ?? 300) + 40;
    const height = (graph.height ?? 200) + 40;

    return { nodes, edges: edgeLines, width, height };
  }, [edges, files]);

  if (!layout || files.length === 0) {
    return (
      <div className="p-4 text-text-dim text-sm">
        Add multiple files to see import graph.
      </div>
    );
  }

  if (edges.length === 0) {
    return (
      <div className="p-4 text-text-dim text-sm">
        No imports detected between files.
      </div>
    );
  }

  return (
    <div className="h-full overflow-auto p-2">
      <svg
        width={layout.width}
        height={layout.height}
        className="mx-auto"
      >
        <defs>
          <marker
            id="arrowhead"
            markerWidth="10"
            markerHeight="7"
            refX="10"
            refY="3.5"
            orient="auto"
          >
            <polygon points="0 0, 10 3.5, 0 7" fill="#6c7086" />
          </marker>
        </defs>

        {layout.edges.map((e, i) => {
          const points = e.points
            .map((p) => `${p.x},${p.y}`)
            .join(" ");
          return (
            <polyline
              key={i}
              points={points}
              fill="none"
              stroke="#6c7086"
              strokeWidth={1.5}
              markerEnd="url(#arrowhead)"
            />
          );
        })}

        {layout.nodes.map((n) => (
          <g key={n.id}>
            <rect
              x={n.x - n.width / 2}
              y={n.y - n.height / 2}
              width={n.width}
              height={n.height}
              rx={6}
              fill="#313244"
              stroke="#89b4fa"
              strokeWidth={1}
            />
            <text
              x={n.x}
              y={n.y + 4}
              textAnchor="middle"
              fill="#cdd6f4"
              fontSize={12}
              fontFamily="monospace"
            >
              {n.id}
            </text>
          </g>
        ))}
      </svg>
    </div>
  );
}
