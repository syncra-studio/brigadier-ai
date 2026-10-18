import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";

import { request } from "@/ipc/client";
import type {
  BranchChoice,
  CleanProgress,
  CleanReport,
  FreeUpProjectsReport,
  ProjectId,
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

/** Removes the picked items of a scan; `onCleanProgress` hears how it goes. */
export async function cleanStorage(scanId: string, items: string[]): Promise<CleanReport> {
  return (await request({ method: "cleanStorage", scanId, items })).report;
}

const cleanListeners = new Set<(progress: CleanProgress) => void>();

/** How each clean goes from now on, after each item; returns the unsubscribe. */
export function onCleanProgress(listener: (progress: CleanProgress) => void): () => void {
  cleanListeners.add(listener);
  return () => {
    cleanListeners.delete(listener);
  };
}

export function emitCleanProgress(progress: CleanProgress): void {
  for (const listener of cleanListeners) listener(progress);
}

/**
 * Frees up projects not in use: their conversations are deleted and their Brains go to the
 * Trash; the projects, their repositories, files and branches stay.
 */
export async function freeUpProjects(ids: ProjectId[]): Promise<FreeUpProjectsReport> {
  return (await request({ method: "freeUpProjects", ids })).report;
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

/**
 * Removes the dictation speech model through the same scan and clean as Free up space, which
 * keeps it while a dictation or its download runs. Resolves when it is gone (or wasn't there).
 */
export async function removeSpeechModel(): Promise<void> {
  const report = await scanStorage();
  const model = report.items.find(
    (item) => item.category === "models" && /[\\/]models[\\/]whisper[\\/]?$/.test(item.path ?? ""),
  );
  if (!model) return;
  if (!model.selectable) throw new Error("Dictation is using it now. Try again when it has finished.");
  const cleaned = await cleanStorage(report.scanId, [model.id]);
  const failed = cleaned.failures[0];
  if (failed) throw new Error(failed.error);
}
