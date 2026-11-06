// What a web page may have without asking anyone. Everything else is refused until the prompt from
// section 4 of the browser work exists: no handler at all would mean Electron grants every request.
import type { Session } from "electron";

import type { Note } from "./events";

const ALLOWED = new Set<string>(["fullscreen", "clipboard-sanitized-write", "pointerLock"]);

/** The app's own session: its page keeps what it asks for (the microphone for dictation), and nothing else in it, a visual's frame say, gets anything. */
export function applyAppSessionPolicy(session: Session, { note, isAppPage }: { note: Note; isAppPage: (url: string) => boolean }): void {
  session.setPermissionRequestHandler((contents, permission, callback, details) => {
    const origin = details.requestingUrl || contents.getURL();
    const allowed = details.isMainFrame && isAppPage(origin);
    note("permission-request", { permission, allowed, origin, session: "app" });
    callback(allowed);
  });
  session.setPermissionCheckHandler((_contents, _permission, origin) => isAppPage(origin));
}

export function applyPermissionPolicy(session: Session, { note }: { note: Note }): void {
  session.setPermissionRequestHandler((contents, permission, callback, details) => {
    const allowed = ALLOWED.has(permission);
    note("permission-request", { permission, allowed, origin: details.requestingUrl || contents.getURL() });
    callback(allowed);
  });
  session.setPermissionCheckHandler((_contents, permission) => ALLOWED.has(permission));
  session.setDevicePermissionHandler(() => false);
  session.setDisplayMediaRequestHandler((_request, callback) => callback({} as never));
  session.on("select-hid-device", (event, _details, callback) => { event.preventDefault(); callback(""); });
  session.on("select-serial-port", (event, _ports, _contents, callback) => { event.preventDefault(); callback(""); });
  session.on("select-usb-device", (event, _details, callback) => { event.preventDefault(); callback(""); });
}
