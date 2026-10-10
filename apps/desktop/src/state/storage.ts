import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";

import { request } from "@/ipc/client";
import type {
  BranchChoice,
  CleanReport,
  StorageReport,
  UninstallApp,
  UninstallPlan,
  UninstallReport,
} from "@/ipc/generated";

/**
 * Settings → Storage (Free up space) and Uninstall Brigadier…: the daemon scans, cleans and uninstalls; the
 * app previews and confirms, and picks items only by the ids a scan or a preview gave them.
 */

export const useStorageUi = create<{ uninstallOpen: boolean }>(() => ({
  uninstallOpen: false,
}));

export function openUninstall(): void {
  useStorageUi.setState({ uninstallOpen: true });
}

export function closeStorageDialogs(): void {
  useStorageUi.setState({ uninstallOpen: false });
}

/**
 * What Brigadier keeps on disk and what it can clean up. The daemon is told which app asks, so
 * that app's own caches are never offered (without it, no app's caches are).
 */
export async function scanStorage(): Promise<StorageReport> {
  const app = await invoke<UninstallApp>("uninstall_app").then(
    (found) => found.identifier,
    () => undefined,
  );
  return (await request(app ? { method: "scanStorage", app } : { method: "scanStorage" })).report;
}

/** Removes the picked items of a scan. */
export async function cleanStorage(scanId: string, items: string[]): Promise<CleanReport> {
  return (await request({ method: "cleanStorage", scanId, items })).report;
}

/** What uninstalling this app removes. */
export async function previewUninstall(): Promise<UninstallPlan> {
  const app = await invoke<UninstallApp>("uninstall_app");
  return (await request({ method: "previewUninstall", app })).plan;
}

export async function uninstall(
  planId: string,
  keepData: boolean,
  deleteBranches: BranchChoice[],
): Promise<UninstallReport> {
  return (await request({ method: "uninstall", planId, keepData, deleteBranches })).report;
}

/** Quits the app; the daemon drains and quits first. */
export function quitApp(): Promise<void> {
  return invoke("quit_app");
}
