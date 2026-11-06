// The system's side of a site permission, after the person's yes to Brigadier's card: its own question, then its
// answer. Electron doesn't ask for location: Chromium only starts position updates, so Brigadier asks macOS itself
// and answers the page once macOS has. Linux has no location: Chromium goes to Google's paid service there, which
// would need a key in the app (Electron's GOOGLE_API_KEY), so a page is told no at once.
import { shell, systemPreferences } from "electron";

import { native } from "./native";

import type { SiteAsk } from "./profiles";

export type OsDevice = "camera" | "microphone" | "location";

const SETTINGS_PANE: Record<OsDevice, string> = {
  camera: "Privacy_Camera",
  microphone: "Privacy_Microphone",
  location: "Privacy_LocationServices",
};

/** How long a person has to answer macOS's location question before the page is told no. */
const ANSWER_MS = 120_000;
const READ_MS = 250;

export const locationAvailable = (): boolean => process.platform !== "linux";

/** What the system says now, read without asking: for the log, and for a check to know a press won't raise its question. */
export function osStatus(asked: SiteAsk): Partial<Record<OsDevice, string>> {
  if (process.platform !== "darwin") return {};
  return {
    ...(asked.camera ? { camera: systemPreferences.getMediaAccessStatus("camera") } : {}),
    ...(asked.microphone ? { microphone: systemPreferences.getMediaAccessStatus("microphone") } : {}),
    ...(asked.location ? { location: native().locationStatus() } : {}),
  };
}

async function locationAllowed(): Promise<boolean> {
  let status = native().locationStatus();
  if (status === "undetermined") {
    native().locationAsk();
    const until = Date.now() + ANSWER_MS;
    while (status === "undetermined" && Date.now() < until) {
      await new Promise((resolve) => setTimeout(resolve, READ_MS));
      status = native().locationStatus();
    }
  }
  return status === "allowed";
}

/** Asks the system for what the person just allowed; what it refuses, has off, or leaves unanswered comes back by name. */
export async function refusedByOs(asked: SiteAsk): Promise<OsDevice[]> {
  if (process.platform !== "darwin") return [];
  const refused: OsDevice[] = [];
  for (const device of ["camera", "microphone"] as const) {
    if (asked[device] && !(await systemPreferences.askForMediaAccess(device))) refused.push(device);
  }
  if (asked.location && !(await locationAllowed())) refused.push("location");
  return refused;
}

export function openOsSettings(device: OsDevice): void {
  if (process.platform !== "darwin") return;
  void shell.openExternal(`x-apple.systempreferences:com.apple.preference.security?${SETTINGS_PANE[device]}`);
}
