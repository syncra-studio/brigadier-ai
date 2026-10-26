import { invoke } from "@tauri-apps/api/core";
import { useEffect, useSyncExternalStore } from "react";

import type { ComputerPermission, ComputerSession, ComputerUseStatus } from "@/ipc/generated";
import { request } from "@/ipc/client";

/**
 * Computer use as Settings, the setup window and the menu bar show it. Reading it never starts
 * the helper: brigadierd answers from a helper already running, or from what it last wrote.
 * Only permissions nothing has read yet are checked once ([`checkUnreadOnce`]). Changes arrive
 * as `computerUse` bridge events.
 */

let latest: ComputerUseStatus | null = null;
const listeners = new Set<() => void>();

function set(next: ComputerUseStatus): void {
  latest = next;
  notify();
}

function subscribeStore(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** A status brigadierd sent (the bridge's `computerUse` event). */
export function onComputerUse(next: ComputerUseStatus): void {
  set(next);
}

/** Reads computer use again. Starts nothing. */
export async function refreshComputerUse(): Promise<ComputerUseStatus> {
  const { computerUse } = await request({ method: "getComputerUse" });
  set(computerUse);
  return computerUse;
}

/** Computer use, read once when first shown and kept up to date. Null until it is read. */
export function useComputerUse(): ComputerUseStatus | null {
  const current = useSyncExternalStore(subscribeStore, () => latest);
  useEffect(() => {
    void refreshComputerUse().catch(() => {});
  }, []);
  return current;
}

/** Asks macOS for `permission` on the helper's behalf. */
export async function grantComputerPermission(permission: ComputerPermission): Promise<void> {
  await request({ method: "grantComputerPermission", permission });
  await refreshComputerUse();
}

/** Checks the permissions again (Check again). */
export async function checkComputerUse(): Promise<ComputerUseStatus> {
  const { computerUse } = await request({ method: "checkComputerUse" });
  set(computerUse);
  return computerUse;
}

let firstCheck: Promise<unknown> | null = null;
let checkingUnread = false;

function notify(): void {
  for (const listener of listeners) listener();
}

/**
 * The permissions as the helper holds them, when nothing has read them yet (no helper has run
 * for this data folder): one check per run of the app, as Check again does.
 */
export function checkUnreadOnce(): void {
  if (firstCheck) return;
  checkingUnread = true;
  notify();
  firstCheck = checkComputerUse()
    .catch(() => {})
    .finally(() => {
      checkingUnread = false;
      notify();
    });
}

/** Whether that one check runs now. */
export function useCheckingUnread(): boolean {
  return useSyncExternalStore(subscribeStore, () => checkingUnread);
}

/** Takes one test picture; `problem` says why it didn't work, when it didn't. */
export async function confirmComputerCapture(): Promise<string | null> {
  const { computerUse, problem } = await request({ method: "confirmComputerCapture" });
  set(computerUse);
  return problem;
}

/** Ends an agent's control of the app in `session`. */
export async function stopComputerSession(session: ComputerSession): Promise<void> {
  await request({
    method: "stopComputerSession",
    session: session.session,
    bundleId: session.bundleId || null,
    app: session.app,
  });
}

/** Opens System Settings at the list that grants `permission`. */
export function openPrivacySettings(permission: ComputerPermission): Promise<void> {
  return invoke("open_privacy_settings", { permission });
}

/** Opens the setup window, or brings it forward. */
export function openComputerSetup(): Promise<void> {
  return invoke("open_computer_setup");
}

/** The permissions in the order setup asks for them. */
export const PERMISSIONS: readonly ComputerPermission[] = ["accessibility", "screenRecording"];

/** A permission's name, as System Settings lists it. */
export function permissionName(permission: ComputerPermission): string {
  return permission === "accessibility" ? "Accessibility" : "Screen Recording";
}

/** What computer use calls this computer: "this Mac" on macOS. */
export function thisComputer(platform: ComputerUseStatus["platform"] | undefined): string {
  return platform === "macos" || platform === undefined ? "this Mac" : "this computer";
}

/** One line on where setup stands. */
export function setupLine(status: ComputerUseStatus): string {
  const setup = status.setup;
  const here = thisComputer(status.platform);
  switch (setup.state) {
    case "off":
      return `Off. Agents can't use apps on ${here}.`;
    case "notSetUp":
      return "Not set up yet. It starts the first time an agent needs it.";
    case "missing": {
      // Windows and Linux grant nothing: what's missing is the desktop session's.
      if (status.platform !== "macos") return status.notes[0] ?? `Agents can't use apps on ${here} right now.`;
      const missing = PERMISSIONS.filter((permission) => permissionState(status, permission) !== "granted").map(permissionName);
      return missing.length === 2
        ? "Almost done: Accessibility and Screen Recording aren't allowed yet."
        : `Almost done: ${missing[0] ?? "a permission"} isn't allowed yet.`;
    }
    case "confirmCapture":
      return "Almost done: Brigadier still needs to check that it can see app windows.";
    case "ready":
      return status.current ? `Ready. Agents can use apps on ${here}.` : "Set up. It starts when an agent needs it.";
    case "restarting":
      return "The helper isn't responding. Brigadier is restarting it.";
    case "failed":
      return `Computer use isn't working right now: ${setup.message}`;
  }
}

/** Whether setup has something left for the user to do in the setup window (macOS only). */
export function setupUnfinished(status: ComputerUseStatus): boolean {
  return status.platform === "macos" && ["notSetUp", "missing", "confirmCapture", "failed"].includes(status.setup.state);
}

export function permissionState(status: ComputerUseStatus, permission: ComputerPermission) {
  return permission === "accessibility" ? status.accessibility : status.screenRecording;
}
