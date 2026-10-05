/** Dragged dimensions belong to the app, so every chat uses the same pane sizes. */
export const PANE_SIZE_KEY = "brigadier.paneSizes";
export type PaneKind =
  | "workers"
  | "review"
  | "terminal"
  | "browser"
  | "browserComposer"
  | "files"
  | "source"
  | "sideChat";
export type PaneSizes = Partial<Record<PaneKind, number>>;
const PANES: readonly string[] = [
  "workers",
  "review",
  "terminal",
  "browser",
  "browserComposer",
  "files",
  "source",
  "sideChat",
];

export function savedPaneSizes(): PaneSizes {
  try {
    const value: unknown = JSON.parse(
      localStorage.getItem(PANE_SIZE_KEY) ?? "{}",
    );
    if (!value || typeof value !== "object") return {};
    return Object.fromEntries(
      Object.entries(value).filter(
        ([key, size]) =>
          PANES.includes(key) &&
          typeof size === "number" &&
          Number.isFinite(size) &&
          size > 0,
      ),
    );
  } catch {
    return {};
  }
}

export function changedPaneSize(
  current: PaneSizes,
  pane: PaneKind,
  size: number | null,
): PaneSizes {
  const sizes = { ...current };
  if (size === null) delete sizes[pane];
  else if (Number.isFinite(size) && size > 0) sizes[pane] = size;
  try {
    localStorage.setItem(PANE_SIZE_KEY, JSON.stringify(sizes));
  } catch {
    /* Keep in memory. */
  }
  return sizes;
}
