import { create } from "zustand";

import { openSettings } from "@/state/actions";
import { type Selection, useApp } from "@/state/store";

/** A sub-view of Settings › Browser. */
export type BrowserSubView = "history" | "downloads" | "sites";

/** A part of Settings › Browser: a sub-view, or the Clear browsing data dialog over the page. */
export type BrowserSettingsView = BrowserSubView | "clear";

/**
 * What Settings › Browser shows over its page: a sub-view, and whether Clear browsing data is
 * open. Each lasts while Settings shows what it was opened under, so Browser picked again in
 * the sidebar opens the page itself. Kept here, not in the page, so the Browser tab's ⋯ menu
 * can open them too.
 */
export const useBrowserSettingsView = create<{
  opened: { view: BrowserSubView; under: Selection } | null;
  clearing: Selection | null;
}>(() => ({ opened: null, clearing: null }));

/** Shows `view` (or the page itself) under what Settings shows now. */
export function showBrowserSettingsView(view: BrowserSettingsView | "page"): void {
  const under = useApp.getState().selection;
  if (view === "clear") useBrowserSettingsView.setState({ clearing: under });
  else useBrowserSettingsView.setState({ opened: view === "page" ? null : { view, under } });
}

/** Opens or closes the Clear browsing data dialog. */
export function setClearingBrowsingData(open: boolean): void {
  if (open) showBrowserSettingsView("clear");
  else useBrowserSettingsView.setState({ clearing: null });
}

/** Shows Settings › Browser, at `view` when given. */
export function openBrowserSettings(view?: BrowserSettingsView): void {
  openSettings("browser");
  showBrowserSettingsView(view ?? "page");
}
