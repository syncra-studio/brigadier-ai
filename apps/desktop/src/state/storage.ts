import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";

import { request } from "@/ipc/client";
import { formatBytes } from "@/lib/format";
import { toast } from "@/state/toasts";
import type {
  BranchChoice,
  CleanReport,
  StorageReport,
  UninstallApp,
  UninstallPlan,
  UninstallReport,
} from "@/ipc/generated";

/**
 * Settings → Storage and Uninstall Brigadier…: the daemon scans, cleans and uninstalls; the
 * app previews and confirms, and picks items only by the ids a scan or a preview gave them.
 */

export const useStorageUi = create<{ storageOpen: boolean; uninstallOpen: boolean }>(() => ({
  storageOpen: false,
  uninstallOpen: false,
}));

export function openStorage(): void {
  useStorageUi.setState({ storageOpen: true, uninstallOpen: false });
}

export function openUninstall(): void {
  useStorageUi.setState({ storageOpen: false, uninstallOpen: true });
}

export function closeStorageDialogs(): void {
  useStorageUi.setState({ storageOpen: false, uninstallOpen: false });
}

/** What Brigadier keeps on disk and what it can clean up. */
export async function scanStorage(): Promise<StorageReport> {
  return (await request({ method: "scanStorage" })).report;
}

/** Removes the picked items of a scan. */
export async function cleanStorage(scanId: string, items: string[]): Promise<CleanReport> {
  return (await request({ method: "cleanStorage", scanId, items })).report;
}

/** Runs Storage's "Compact the database" on its own, and says how it went. */
export async function compactDatabase(): Promise<void> {
  try {
    const report = await scanStorage();
    const item = report.items.find((entry) => entry.category === "database" && entry.selectable);
    if (!item) {
      toast("The database is already compact.");
      return;
    }
    const cleaned = await cleanStorage(report.scanId, [item.id]);
    const failed = cleaned.failures.map((failure) => failure.error).join("; ");
    if (failed) {
      toast(`Couldn't compact the database: ${failed}`, { tone: "error" });
    } else {
      toast(`Compacted the database: ${formatBytes(cleaned.reclaimedBytes)} given back.`);
    }
  } catch (cause) {
    const why = cause instanceof Error ? cause.message : String(cause);
    toast(`Couldn't compact the database: ${why}`, { tone: "error" });
  }
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
