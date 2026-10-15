import type { NodeKind, NodeState, Origin } from "@/ipc/generated";

/** Every node kind, in the order the filters and legends list them. */
export const NODE_KINDS: readonly NodeKind[] = [
  "module",
  "service",
  "fileSummary",
  "decision",
  "convention",
  "preference",
  "contract",
  "constraint",
  "task",
  "report",
  "research",
];

/**
 * Each kind's label and color: a Tailwind class for the DOM and the token it reads, which the
 * graph viewer resolves for WebGL. File summaries, the most numerous, are a recessive neutral.
 */
export const KINDS: Record<NodeKind, { label: string; swatch: string; token: `--${string}` }> = {
  module: { label: "Module", swatch: "bg-glyph-6", token: "--glyph-6" },
  service: { label: "Service", swatch: "bg-glyph-2", token: "--glyph-2" },
  fileSummary: { label: "File summary", swatch: "bg-muted-foreground", token: "--muted-foreground" },
  decision: { label: "Decision", swatch: "bg-glyph-5", token: "--glyph-5" },
  convention: { label: "Convention", swatch: "bg-glyph-4", token: "--glyph-4" },
  preference: { label: "Preference", swatch: "bg-glyph-1", token: "--glyph-1" },
  contract: { label: "Contract", swatch: "bg-chart-3", token: "--chart-3" },
  constraint: { label: "Constraint", swatch: "bg-chart-1", token: "--chart-1" },
  task: { label: "Task", swatch: "bg-glyph-7", token: "--glyph-7" },
  report: { label: "Report", swatch: "bg-glyph-8", token: "--glyph-8" },
  research: { label: "Research", swatch: "bg-glyph-3", token: "--glyph-3" },
};

export const ORIGIN_LABELS: Record<Origin, string> = {
  index: "code index",
  skeleton: "skeleton pass",
  enrichment: "enrichment",
  report: "worker report",
  orchestrator: "orchestrator",
  user: "user",
};

export function stateLabel(state: NodeState): string | null {
  switch (state.type) {
    case "fresh":
      return null;
    case "stale":
      return "stale";
    case "superseded":
      return "superseded";
  }
}
