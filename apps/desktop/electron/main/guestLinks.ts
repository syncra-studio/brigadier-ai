// mailto: and tel: links in a page go to the OS's default app, as in a normal browser, when the user just pressed
// something. The press comes from the browser's own input stream, which a page can't write to; one press launches
// once. Every other scheme stays refused: Electron asks the session's permission handler, which says no. And a
// page can't go to the app's own schemes or a local one, whichever frame asks. `will-frame-navigate` doesn't see
// a load the app starts itself; those take web addresses only (browserAddress.ts, browser_tool.rs's web_address).
import type { WebContents } from "electron";

import { openWithOs } from "./dialogs";
import { note } from "./events";
import { unfocused } from "./identity";
import { refusedAddress } from "./pagePolicy";

/** How long after a press a link's load still counts as that press. */
const GESTURE_MS = 1000;
const PRESSES = new Set(["mouseUp", "rawKeyDown", "keyDown", "gestureTap"]);

const pressedAt = new WeakMap<WebContents, number>();

const isOsLink = (url: string) => /^(mailto|tel):/i.test(url);

/** Takes a mailto: or tel: address, launching it only on a fresh press; false for any other address. */
export function takeOsLink(contents: WebContents, url: string): boolean {
  if (!isOsLink(url)) return false;
  const pressed = pressedAt.get(contents);
  pressedAt.delete(contents);
  if (pressed === undefined || Date.now() - pressed > GESTURE_MS) {
    note("guest-os-link-refused", { webContents: contents.id, url });
    return true;
  }
  // A dev run with no stub for the launch must not start the user's mail app (L8).
  if (unfocused && !process.env.BRIGADIER_DEV_EFFECTS) note("guest-os-link-not-launched", { webContents: contents.id, url });
  else openWithOs(url).catch((error: unknown) => note("guest-os-link-failed", { webContents: contents.id, url, error: String(error) }));
  return true;
}

function refuse(contents: WebContents, url: string): boolean {
  if (!refusedAddress(url)) return false;
  note("guest-navigation-refused", { webContents: contents.id, url });
  return true;
}

export function installGuestLinks(contents: WebContents): void {
  contents.on("input-event", (_event, input) => {
    if (PRESSES.has(input.type)) pressedAt.set(contents, Date.now());
  });
  contents.on("will-frame-navigate", (details) => {
    if (takeOsLink(contents, details.url) || refuse(contents, details.url)) details.preventDefault();
  });
  contents.on("will-redirect", (details) => {
    if (refuse(contents, details.url)) details.preventDefault();
  });
}
