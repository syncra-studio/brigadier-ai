import { create } from "zustand";

import { openSettings, select } from "@/state/actions";
import { type BrowserSubView, type Selection, useApp } from "@/state/store";

/** A sub-view of Settings › Browser. */
export type { BrowserSubView } from "@/state/store";

/** A part of Settings › Browser: a sub-view, or the Clear browsing data dialog over the page. */
export type BrowserSettingsView = BrowserSubView | "clear";

/**
 * Clear browsing data is a dialog attached to its destination, never a history entry.
 */
export const useBrowserSettingsView = create<{
  clearing: Selection | null;
}>(() => ({ clearing: null }));

/** Shows `view` (or the page itself) under what Settings shows now. */
export function showBrowserSettingsView(view: BrowserSettingsView | "page"): void {
  const under = useApp.getState().selection;
  if (view === "clear") useBrowserSettingsView.setState({ clearing: under });
  else if (view === "page") openSettings("browser");
  else select({ type: "settings", page: "browser", detail: { view } });
}

/** Opens or closes the Clear browsing data dialog. */
export function setClearingBrowsingData(open: boolean): void {
  if (open) showBrowserSettingsView("clear");
  else useBrowserSettingsView.setState({ clearing: null });
}

/** Shows Settings › Browser, at `view` when given. */
export function openBrowserSettings(view?: BrowserSettingsView): void {
  if (!view || view === "clear") openSettings("browser");
  if (view) showBrowserSettingsView(view);
}
