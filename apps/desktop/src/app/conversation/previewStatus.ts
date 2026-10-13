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

/** Prefer the newest active preview; finished previews stay reachable until cleared. */
export function previewChip(previews: Readonly<Record<string, Preview>>) {
  const all = Object.values(previews).toSorted((a, b) => b.startedAtMs - a.startedAtMs || b.id.localeCompare(a.id));
  const newest = all.find((preview) => previewActive(preview.state)) ?? all[0];
  if (!newest) return null;
  const active = previewActive(newest.state);
  return {
    label: newest.name,
    status: active ? previewStateLabel(newest.state) : "Finished",
    active,
    title: all.map((preview) => `${preview.name}: ${preview.command} (in ${preview.workdir})`).join("\n"),
  };
}
