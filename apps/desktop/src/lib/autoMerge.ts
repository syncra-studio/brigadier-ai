/**
 * Auto-merge in the app (WORKSPACES-PLAN.md phase 3): Settings › Git's "Merge on its own"
 * switch. Off until it is finished; `BRIGADIER_AUTO_MERGE=1` at build time turns it on
 * (vite.config.ts, and the engine reads the same variable), and fixtures turn it on themselves.
 */
declare const BRIGADIER_AUTO_MERGE_FLAG: string | undefined;

let on = typeof BRIGADIER_AUTO_MERGE_FLAG !== "undefined" && BRIGADIER_AUTO_MERGE_FLAG === "1";

export function autoMergeOn(): boolean {
  return on;
}

/** For fixtures and tests. */
export function setAutoMergeOn(value: boolean): void {
  on = value;
}
