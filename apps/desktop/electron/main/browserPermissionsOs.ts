// The system's side of a site permission, after the person's yes to Brigadier's card: its own question for the
// camera and microphone, and for location what it says now. Chromium asks macOS for location itself once a page
// has the permission (`CoreLocationProvider`), so here that answer is only read, to name a refusal to the person.
import { shell, systemPreferences } from "electron";

import { native } from "./native";

import type { SiteAsk } from "./profiles";

export type OsDevice = "camera" | "microphone" | "location";

const SETTINGS_PANE: Record<OsDevice, string> = {
  camera: "Privacy_Camera",
  microphone: "Privacy_Microphone",
  location: "Privacy_LocationServices",
};

/** Asks the system for what the person just allowed; what it refuses, or has turned off, comes back by name. */
export async function refusedByOs(asked: SiteAsk): Promise<OsDevice[]> {
  if (process.platform !== "darwin") return [];
  const refused: OsDevice[] = [];
  for (const device of ["camera", "microphone"] as const) {
    if (asked[device] && !(await systemPreferences.askForMediaAccess(device))) refused.push(device);
  }
  if (asked.location && locationRefused()) refused.push("location");
  return refused;
}

export const locationStatus = () => native().locationStatus();

/** "undetermined" is no refusal: Chromium's start of location updates makes macOS ask. */
function locationRefused(): boolean {
  const status = locationStatus();
  return status === "denied" || status === "restricted" || status === "off";
}

export function openOsSettings(device: OsDevice): void {
  if (process.platform !== "darwin") return;
  void shell.openExternal(`x-apple.systempreferences:com.apple.preference.security?${SETTINGS_PANE[device]}`);
}
