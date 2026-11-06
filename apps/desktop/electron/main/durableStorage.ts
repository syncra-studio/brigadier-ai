// What a page writes must outlive a crash, and Chromium holds it back: a localStorage write waits about 5 s to
// commit, and longer for a site that writes often. So the kept-alive state is committed on the moments that
// matter (a page leaving, the app's tabs changing) and on a short beat for the rest, which a commit with nothing
// pending makes free. A kill -9 can still lose what was written in the last FLUSH_EVERY_MS without a navigation.
import fs from "node:fs";

import type { Session, WebContents } from "electron";

import { onFromApp } from "./appPage";

const FLUSH_EVERY_MS = 3000;
const FLUSH_SOON_MS = 150;

const sessions = new Set<Session>();
const soon = new WeakSet<Session>();

function flushSoon(session: Session): void {
  if (soon.has(session)) return;
  soon.add(session);
  setTimeout(() => {
    soon.delete(session);
    session.flushStorageData();
  }, FLUSH_SOON_MS);
}

export function installDurableStorage(): void {
  setInterval(() => {
    for (const session of sessions) session.flushStorageData();
  }, FLUSH_EVERY_MS);
  onFromApp("brigadier:flush-storage", (event) => flushSoon(event.sender.session));
}

/** Each page's storage goes on the beat, and at once when a guest leaves its page: sign-ins write and then go. */
export function keepStorageOf(contents: WebContents): void {
  sessions.add(contents.session);
  if (contents.getType() !== "webview") return;
  contents.on("did-start-navigation", (details) => {
    if (details.isMainFrame && !details.isSameDocument) flushSoon(contents.session);
  });
}

/** Replaces `file` whole or not at all: a crash mid-write must leave the old contents, not half of the new. */
export function replaceFile(file: string, contents: string): void {
  const temporary = `${file}.tmp`;
  const handle = fs.openSync(temporary, "w");
  try {
    fs.writeFileSync(handle, contents);
    fs.fsyncSync(handle);
  } finally {
    fs.closeSync(handle);
  }
  fs.renameSync(temporary, file);
}
