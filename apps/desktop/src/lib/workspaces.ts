/**
 * Workspaces in the app (WORKSPACES-PLAN.md phase 2): task tabs in the strip, workspace rows in
 * the sidebar, ⌘T and ⌘⏎. Off until they are finished; `BRIGADIER_WORKSPACES=1` at build time
 * turns them on (vite.config.ts), and fixtures turn them on themselves.
 */
declare const BRIGADIER_WORKSPACES_FLAG: string | undefined;

let on = typeof BRIGADIER_WORKSPACES_FLAG !== "undefined" && BRIGADIER_WORKSPACES_FLAG === "1";

export function workspacesOn(): boolean {
  return on;
}

/** For fixtures and tests. */
export function setWorkspacesOn(value: boolean): void {
  on = value;
}
