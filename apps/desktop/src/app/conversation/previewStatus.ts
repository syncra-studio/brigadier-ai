import type { PreviewState } from "@/ipc/generated";

export function previewActive(state: PreviewState): boolean {
  return state.type === "running" || state.type === "paused";
}

export function previewStateLabel(state: PreviewState): string {
  switch (state.type) {
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
