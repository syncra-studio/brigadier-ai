export const COLLAPSED_KEY = "brigadier.sidebarCollapsed";
export const COLLAPSE_MODE_KEY = "brigadier.sidebarCollapseMode";
export type SidebarCollapseMode = "strip" | "hidden";

export function cachedOpen(): boolean {
  try {
    return localStorage.getItem(COLLAPSED_KEY) !== "1";
  } catch {
    return true;
  }
}

export function saveOpen(open: boolean): void {
  try {
    if (open) localStorage.removeItem(COLLAPSED_KEY);
    else localStorage.setItem(COLLAPSED_KEY, "1");
  } catch {
    // Storage can be unavailable; the sidebar then opens expanded on the next launch.
  }
}

export function cachedCollapseMode(): SidebarCollapseMode {
  try {
    return localStorage.getItem(COLLAPSE_MODE_KEY) === "hidden" ? "hidden" : "strip";
  } catch {
    return "strip";
  }
}

export function saveCollapseMode(mode: SidebarCollapseMode): void {
  try {
    localStorage.setItem(COLLAPSE_MODE_KEY, mode);
  } catch {
    // Storage can be unavailable; the mode then lasts until the app quits.
  }
}

export function cachedWidth(key: string): number | null {
  try {
    const width = Number(localStorage.getItem(key));
    return Number.isFinite(width) && width > 0 ? width : null;
  } catch {
    return null;
  }
}

export function saveWidth(key: string, width: number | null): void {
  try {
    if (width === null) localStorage.removeItem(key);
    else localStorage.setItem(key, String(Math.round(width)));
  } catch {
    // Storage can be unavailable; the width then lasts until the app quits.
  }
}
