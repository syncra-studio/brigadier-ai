import type { AgentState } from "@/components/assistant-ui/elements/agent-status";
import type { Preview, PreviewState } from "@/ipc/generated";

export function previewActive(state: PreviewState): boolean {
  return state.type === "starting" || state.type === "running" || state.type === "paused";
}

export function previewStateLabel(state: PreviewState): string {
  switch (state.type) {
    case "starting": return "Starting";
    case "running": return "Running";
    case "paused": return "Paused";
    case "exited": return state.code === null ? `Exited · ${state.status}` : `Exited · code ${state.code}`;
    case "stopped": return `Stopped · ${state.reason}`;
  }
}

export function previewActions(state: PreviewState, platform: string): ("Pause" | "Resume" | "Stop")[] {
  if (!previewActive(state)) return [];
  const suspend = platform === "macos" || platform === "linux";
  return suspend ? [state.type === "paused" ? "Resume" : "Pause", "Stop"] : ["Stop"];
}

// Escape sequences: CSI (colors, erase line), OSC (titles, links) and single-character ones.
// eslint-disable-next-line no-control-regex
const ESCAPES = /\u001b(?:\[[0-?]*[ -/]*[@-~]|\][^\u0007\u001b]*(?:\u0007|\u001b\\)|[@-_])/g;

/** A log tail's latest line as a terminal shows it: escape codes gone, and a line redrawn with
 * carriage returns (cargo's progress bar) as its last drawing. */
export function latestLine(tail: string): string {
  const lines = tail.replace(ESCAPES, "").split("\n").map((line) => {
    const drawn = line.split("\r").map((part) => part.trim()).filter(Boolean);
    return drawn.at(-1) ?? "";
  });
  return lines.findLast(Boolean) ?? "";
}

/** A preview's state in a word. A web page that is up is showing; an app, a simulator, a PDF or a test run is running. */
export function previewWord(preview: Preview): string {
  const { state } = preview;
  switch (state.type) {
    case "starting": return preview.building ? "Building" : "Starting";
    case "running": return preview.kind === "web" ? "Showing" : "Running";
    case "paused": return "Paused";
    case "stopped": return "Stopped";
    case "exited": return state.code === 0 ? "Stopped" : "Failed";
  }
}

export function previewIndicator(state: PreviewState): AgentState {
  if (state.type === "exited") return state.code === 0 ? "done" : "failed";
  if (state.type === "stopped") return "done";
  return state.type === "paused" ? "waiting" : "working";
}

/** Every preview contributes to the inbox summary, including paused and ended processes. */
export function previewSummary(previews: readonly Preview[]): string {
  const counts = { starting: 0, running: 0, paused: 0, finished: 0, failed: 0, stopped: 0 };
  for (const { state } of previews) {
    const label = state.type === "exited" ? (state.code === 0 ? "finished" : "failed") : state.type;
    counts[label]++;
  }
  return Object.entries(counts).filter(([, count]) => count > 0)
    .map(([label, count]) => `${count} ${label}`).join(" · ");
}
