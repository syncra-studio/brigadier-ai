import { MultiDirectedGraph } from "graphology";
import forceAtlas2 from "graphology-layout-forceatlas2";
import { useEffect, useLayoutEffect, useRef } from "react";
import { Sigma } from "sigma";
import type { Settings } from "sigma/settings";
import type { NodeDisplayData, PartialButFor } from "sigma/types";

import { KINDS, NODE_KINDS } from "@/app/inspector/brain/kinds";
import type { BrainGraph, NodeKind } from "@/ipc/generated";
import { tokenColor, tokenPx } from "@/lib/tokens";

type NodeAttributes = {
  x: number;
  y: number;
  size: number;
  label: string;
  color: string;
  kind: NodeKind;
  stale: boolean;
  superseded: boolean;
};

type Palette = {
  kinds: Record<NodeKind, string>;
  background: string;
  foreground: string;
  edge: string;
  warning: string;
  popover: string;
  border: string;
};

function readPalette(): Palette {
  const kinds = Object.fromEntries(NODE_KINDS.map((kind) => [kind, tokenColor(KINDS[kind].token)])) as Record<
    NodeKind,
    string
  >;
  const muted = tokenColor("--muted-foreground");
  return {
    kinds,
    background: tokenColor("--background"),
    foreground: tokenColor("--foreground"),
    // Edges are a faint neutral so the nodes' kinds carry the color.
    edge: `${muted.slice(0, 7)}59`,
    warning: tokenColor("--warning"),
    popover: tokenColor("--popover"),
    border: tokenColor("--border"),
  };
}

function channel(hex: string, index: number): number {
  return Number.parseInt(hex.slice(1 + index * 2, 3 + index * 2), 16);
}

/** `a` blended toward `b` by `t` (0: a, 1: b), both `#rrggbb[aa]`, as opaque `#rrggbb`. */
function mix(a: string, b: string, t: number): string {
  const blended = [0, 1, 2].map((index) =>
    Math.round(channel(a, index) * (1 - t) + channel(b, index) * t)
      .toString(16)
      .padStart(2, "0"),
  );
  return `#${blended.join("")}`;
}

/** A stable pseudo-random number in [0, 1) from a node id, so layouts start the same way. */
function hashUnit(id: string): number {
  let hash = 2166136261;
  for (let index = 0; index < id.length; index += 1) {
    hash ^= id.charCodeAt(index);
    hash = Math.imul(hash, 16777619);
  }
  return (hash >>> 0) / 4294967296;
}

function buildGraph(
  data: BrainGraph,
  palette: Palette,
  unit: number,
): MultiDirectedGraph<NodeAttributes> {
  const graph = new MultiDirectedGraph<NodeAttributes>();
  for (const node of data.nodes) {
    // Each kind starts in its own sector of a circle; the layout pulls linked nodes together.
    const sector = NODE_KINDS.indexOf(node.kind) / NODE_KINDS.length;
    const angle = (sector + hashUnit(node.id) / NODE_KINDS.length) * Math.PI * 2;
    const radius = 50 + hashUnit(`${node.id}:r`) * 50;
    const superseded = node.state.type === "superseded";
    graph.addNode(node.id, {
      x: Math.cos(angle) * radius,
      y: Math.sin(angle) * radius,
      size: unit,
      label: node.title,
      color: superseded ? mix(palette.kinds[node.kind], palette.background, 0.7) : palette.kinds[node.kind],
      kind: node.kind,
      stale: node.state.type === "stale",
      superseded,
    });
  }
  for (const edge of data.edges) {
    if (graph.hasNode(edge.from) && graph.hasNode(edge.to)) {
      graph.addEdge(edge.from, edge.to, { kind: edge.kind });
    }
  }
  sizeNodes(graph, unit);
  return graph;
}

/** Better-linked nodes are bigger; `unit` is the spacing token. */
function sizeNodes(graph: MultiDirectedGraph<NodeAttributes>, unit: number) {
  graph.forEachNode((id) => {
    graph.setNodeAttribute(id, "size", unit * (1 + Math.sqrt(graph.degree(id)) * 0.6));
  });
}

type LabelData = PartialButFor<NodeDisplayData, "x" | "y" | "size" | "label" | "color"> & {
  stale?: boolean;
};

function labelDrawer(palette: Palette) {
  return (context: CanvasRenderingContext2D, data: LabelData, settings: Settings<NodeAttributes>) => {
    // Stale nodes carry a ring in the warning color (their labels are always drawn).
    if (data.stale) {
      context.beginPath();
      context.arc(data.x, data.y, data.size + 2, 0, Math.PI * 2);
      context.lineWidth = 1.5;
      context.strokeStyle = palette.warning;
      context.stroke();
    }
    if (!data.label) return;
    context.fillStyle = settings.labelColor.color ?? palette.foreground;
    context.font = `${settings.labelWeight} ${settings.labelSize}px ${settings.labelFont}`;
    context.fillText(data.label, data.x + data.size + 3, data.y + settings.labelSize / 3);
  };
}

function hoverDrawer(palette: Palette) {
  const drawLabel = labelDrawer(palette);
  return (context: CanvasRenderingContext2D, data: LabelData, settings: Settings<NodeAttributes>) => {
    const size = settings.labelSize;
    context.font = `${settings.labelWeight} ${size}px ${settings.labelFont}`;
    const padding = 3;
    const width = data.label ? context.measureText(data.label).width + data.size + padding * 3 : 0;
    const height = size + padding * 2;
    context.fillStyle = palette.popover;
    context.strokeStyle = palette.border;
    context.lineWidth = 1;
    context.beginPath();
    context.arc(data.x, data.y, data.size + padding, 0, Math.PI * 2);
    if (width > 0) context.rect(data.x, data.y - height / 2, width, height);
    context.fill();
    context.stroke();
    drawLabel(context, data, settings);
  };
}

/** Layout iterations in all, and the time each frame may spend on them. */
const LAYOUT_ITERATIONS = 300;
const FRAME_BUDGET_MS = 8;

/**
 * The Brain graph in WebGL (sigma), laid out with ForceAtlas2 a few iterations per frame so no
 * frame runs long. Loaded on demand: only the Brain tab's graph view imports it.
 */
export default function BrainGraphCanvas({
  graph: data,
  selected,
  onSelect,
}: {
  graph: BrainGraph;
  selected: string | null;
  onSelect: (id: string | null) => void;
}) {
  const container = useRef<HTMLDivElement>(null);
  const sigmaRef = useRef<Sigma<NodeAttributes> | null>(null);
  const selectedRef = useRef(selected);
  /** Re-reads the selection into the reducers (set while the graph is shown). */
  const refocusRef = useRef<() => void>(() => {});
  const onSelectRef = useRef(onSelect);

  useLayoutEffect(() => {
    onSelectRef.current = onSelect;
  }, [onSelect]);

  useEffect(() => {
    const element = container.current;
    if (!element) return undefined;
    const palette = readPalette();
    const unit = tokenPx("--spacing");
    const graph = buildGraph(data, palette, unit);
    const neighbors = (id: string) => new Set(graph.neighbors(id));
    let focus: { id: string; around: Set<string> } | null = null;
    const refocus = () => {
      const id = selectedRef.current;
      focus = id && graph.hasNode(id) ? { id, around: neighbors(id) } : null;
    };
    refocus();

    let sigma: Sigma<NodeAttributes> | null = null;
    let frame = 0;
    const draw = () => {
      sigma = new Sigma<NodeAttributes>(graph, element, {
        allowInvalidContainer: true,
        labelFont: getComputedStyle(element).fontFamily,
        labelSize: tokenPx("--text-2xs"),
        labelWeight: "normal",
        labelColor: { color: palette.foreground },
        labelRenderedSizeThreshold: unit * 2,
        defaultEdgeColor: palette.edge,
        defaultDrawNodeLabel: labelDrawer(palette),
        defaultDrawNodeHover: hoverDrawer(palette),
        zIndex: true,
        nodeReducer: (id, attributes) => {
          const shown: Partial<NodeDisplayData> & { stale?: boolean } = { ...attributes };
          if (attributes.stale) shown.forceLabel = true;
          if (focus) {
            if (id === focus.id) {
              shown.highlighted = true;
              shown.zIndex = 2;
            } else if (focus.around.has(id)) {
              shown.forceLabel = true;
              shown.zIndex = 1;
            } else {
              shown.color = mix(attributes.color, palette.background, 0.6);
              shown.label = "";
            }
          }
          return shown;
        },
        edgeReducer: (edge, attributes) => {
          if (!focus) return attributes;
          const touches = graph.source(edge) === focus.id || graph.target(edge) === focus.id;
          return touches ? { ...attributes, color: palette.foreground, zIndex: 1 } : { ...attributes, hidden: true };
        },
      });
      sigmaRef.current = sigma;
      sigma.on("clickNode", ({ node }) => onSelectRef.current(node));
      sigma.on("clickStage", () => onSelectRef.current(null));
      sigma.on("enterNode", () => {
        element.style.cursor = "pointer";
      });
      sigma.on("leaveNode", () => {
        element.style.cursor = "";
      });
      refocusRef.current = refocus;

      // Lay the graph out a few iterations per frame; sigma redraws as positions change.
      const settings = {
        ...forceAtlas2.inferSettings(graph),
        barnesHutOptimize: graph.order > 400,
        // Keeps unlinked nodes near the rest instead of drifting to the edges.
        strongGravityMode: true,
        gravity: 5,
      };
      let done = 0;
      frame = requestAnimationFrame(function step() {
        const started = performance.now();
        while (done < LAYOUT_ITERATIONS && performance.now() - started < FRAME_BUDGET_MS) {
          forceAtlas2.assign(graph, { iterations: 5, settings });
          done += 5;
        }
        if (done < LAYOUT_ITERATIONS && graph.order > 1) frame = requestAnimationFrame(step);
      });
    };
    // Drawn a frame later: creating sigma's WebGL contexts is the costly part, and after a click
    // React runs this effect in the same task as its commit, which together would run past
    // 50 ms.
    frame = requestAnimationFrame(draw);

    return () => {
      cancelAnimationFrame(frame);
      sigma?.kill();
      sigmaRef.current = null;
    };
  }, [data]);

  // Selecting a node focuses it and its neighbors, and moves the camera to it.
  useEffect(() => {
    selectedRef.current = selected;
    const sigma = sigmaRef.current;
    if (!sigma) return;
    refocusRef.current();
    sigma.refresh({ skipIndexation: true });
    if (selected) {
      const shown = sigma.getNodeDisplayData(selected);
      if (shown) {
        const camera = sigma.getCamera();
        void camera.animate({ x: shown.x, y: shown.y, ratio: Math.min(camera.ratio, 0.6) }, { duration: 300 });
      }
    }
  }, [selected]);

  // Sizes are tokens.
  useEffect(() => {
    const sigma = sigmaRef.current;
    if (!sigma) return;
    const unit = tokenPx("--spacing");
    sizeNodes(sigma.getGraph(), unit);
    sigma.setSetting("labelSize", tokenPx("--text-2xs"));
    sigma.setSetting("labelRenderedSizeThreshold", unit * 2);
  }, []);

  return <div ref={container} aria-label="Brain graph" className="absolute inset-0" />;
}
