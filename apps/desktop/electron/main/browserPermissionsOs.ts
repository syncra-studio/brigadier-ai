// The system's side of a site permission, after the person's yes to Brigadier's card: its own question, then its
// answer. Electron doesn't ask for location: Chromium only starts position updates, so Brigadier asks macOS itself
// and answers the page once macOS has.
import { shell, systemPreferences } from "electron";

import { locationAllowed } from "./browserPermissionsRules";
import { native } from "./native";

import type { SiteAsk } from "./profiles";

export type OsDevice = "camera" | "microphone" | "location";

const SETTINGS_PANE: Record<OsDevice, string> = {
  camera: "Privacy_Camera",
  microphone: "Privacy_Microphone",
  location: "Privacy_LocationServices",
};

/** What the system says now, read without asking: for the log, and for a check to know a press won't raise its question. */
export function osStatus(asked: SiteAsk): Partial<Record<OsDevice, string>> {
  if (process.platform !== "darwin") return {};
  return {
    ...(asked.camera ? { camera: systemPreferences.getMediaAccessStatus("camera") } : {}),
    ...(asked.microphone ? { microphone: systemPreferences.getMediaAccessStatus("microphone") } : {}),
    ...(asked.location ? { location: native().locationStatus() } : {}),
  };
}

/** Asks the system for what the person just allowed; what it refuses, has off, or leaves unanswered comes back by name. */
export async function refusedByOs(asked: SiteAsk): Promise<OsDevice[]> {
  if (process.platform !== "darwin") return [];
  const refused: OsDevice[] = [];
  for (const device of ["camera", "microphone"] as const) {
    if (asked[device] && !(await systemPreferences.askForMediaAccess(device))) refused.push(device);
  }
  if (asked.location && !(await locationAllowed(() => native().locationStatus(), () => native().locationAsk()))) refused.push("location");
  return refused;
}

export function openOsSettings(device: OsDevice): void {
  if (process.platform !== "darwin") return;
  void shell.openExternal(`x-apple.systempreferences:com.apple.preference.security?${SETTINGS_PANE[device]}`);
}
