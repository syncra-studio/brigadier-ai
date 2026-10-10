import type { Preview, PreviewState } from "@/ipc/generated";

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

/** Keep finished previews reachable until the user clears them. */
export function previewChip(previews: Readonly<Record<string, Preview>>) {
  const all = Object.values(previews).toSorted((a, b) => b.startedAtMs - a.startedAtMs || b.id.localeCompare(a.id));
  const active = all.filter((preview) => previewActive(preview.state));
  const shown = active.length ? active : all;
  const newest = shown[0];
  if (!newest) return null;
  return {
    label: shown.length === 1 ? newest.name : `${shown.length} previews`,
    status: active.some((preview) => preview.state.type === "running") ? "Running" : active.length ? "Paused" : "Finished",
    title: shown.map((preview) => `${preview.name}: ${preview.command} (in ${preview.workdir})`).join("\n"),
  };
}
