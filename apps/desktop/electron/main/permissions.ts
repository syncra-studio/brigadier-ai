// What a web page may have. The camera and microphone go by the profile's saved choice for the site, then a
// question in the tab; everything else not listed is refused: no handler at all would mean Electron grants it.
import { ipcMain, systemPreferences, type Session, type WebContents } from "electron";

import type { Note } from "./events";
import { isDeleting, setSiteChoice, siteAsked, siteChoice } from "./profileStore";
import { type MediaAsk, mediaAnswer, profileOfStorage } from "./profiles";

const ALLOWED = new Set<string>(["fullscreen", "clipboard-sanitized-write", "pointerLock"]);

type Question = { guest: WebContents; profileId: string; origin: string; asked: MediaAsk; answer: (allowed: boolean) => void };

let nextQuestion = 1;
const questions = new Map<number, Question>();
/** "Allow while visiting": by guest, for as long as that guest stays on the site. Never saved. */
const visiting = new Map<number, { origin: string; devices: MediaAsk }>();
const watched = new WeakSet<WebContents>();

function originOf(url: string): string | null {
  try {
    const { origin } = new URL(url);
    return origin === "null" ? null : origin;
  } catch {
    return null;
  }
}

/** The system's own question comes only after the person's yes to ours: the first thing they see is ours. */
async function allowedByOs(asked: MediaAsk): Promise<boolean> {
  if (process.platform !== "darwin") return true;
  const devices = [asked.camera && "camera", asked.microphone && "microphone"].filter(Boolean) as ("camera" | "microphone")[];
  const answers = await Promise.all(devices.map((device) => systemPreferences.askForMediaAccess(device)));
  return answers.every(Boolean);
}

function grant(asked: MediaAsk, answer: (allowed: boolean) => void): void {
  allowedByOs(asked).then(answer, () => answer(false));
}

function dropQuestions(guest: WebContents, unlessOrigin: string | null): void {
  for (const [request, question] of questions) {
    if (question.guest !== guest || question.origin === unlessOrigin) continue;
    questions.delete(request);
    question.answer(false);
  }
}

/** Leaving the site ends what was allowed while visiting it, and its open question. */
function watchGuest(guest: WebContents): void {
  if (watched.has(guest)) return;
  watched.add(guest);
  const { id } = guest;
  guest.on("did-start-navigation", (details) => {
    if (!details.isMainFrame) return;
    const origin = originOf(details.url);
    dropQuestions(guest, origin);
    if (visiting.get(id)?.origin !== origin) visiting.delete(id);
  });
  guest.once("destroyed", () => {
    dropQuestions(guest, null);
    visiting.delete(id);
  });
}

function visitingFor(guest: WebContents, origin: string): MediaAsk | null {
  const granted = visiting.get(guest.id);
  return granted?.origin === origin ? granted.devices : null;
}

function requestMedia(profileId: string, guest: WebContents, details: Electron.MediaAccessPermissionRequest, answer: (allowed: boolean) => void, note: Note): void {
  const types = details.mediaTypes ?? [];
  const asked: MediaAsk = { camera: types.includes("video"), microphone: types.includes("audio") };
  const origin = originOf(details.requestingUrl);
  const top = originOf(guest.getURL());
  const host = guest.hostWebContents;
  // A question names one site, the tab's: another site's frame asking gets no question, and a page with no tab no door.
  if (!origin || origin !== top || !host || isDeleting(profileId)) {
    note("permission-request", { permission: "media", allowed: false, origin, reason: "not the tab's own site, or the profile is being deleted" });
    answer(false);
    return;
  }
  const decided = mediaAnswer(siteChoice(profileId, origin), asked, visitingFor(guest, origin));
  note("permission-request", { permission: "media", profileId, origin, asked, decided });
  if (decided === "grant") {
    grant(asked, answer);
    return;
  }
  if (decided === "deny") {
    answer(false);
    return;
  }
  siteAsked(profileId, origin);
  watchGuest(guest);
  const request = nextQuestion++;
  questions.set(request, { guest, profileId, origin, asked, answer });
  host.send("brigadier:guest-permission", { from: guest.id, request, origin, ...asked });
}

export function installPermissionAnswers(): void {
  ipcMain.handle("brigadier:permission-answer", (_event, request: number, choice: "once" | "visiting" | "never" | "dismiss") => {
    const question = questions.get(request);
    if (!question) return;
    questions.delete(request);
    const { guest, profileId, origin, asked, answer } = question;
    if (choice === "visiting") {
      const had = visitingFor(guest, origin);
      visiting.set(guest.id, { origin, devices: { camera: asked.camera || (had?.camera ?? false), microphone: asked.microphone || (had?.microphone ?? false) } });
    }
    if (choice === "never") {
      setSiteChoice(profileId, origin, { ...(asked.camera ? { camera: "block" } : {}), ...(asked.microphone ? { microphone: "block" } : {}) });
    }
    if (choice === "once" || choice === "visiting") grant(asked, answer);
    else answer(false);
  });
}

export function applyPermissionPolicy(session: Session, { note }: { note: Note }): void {
  const profileId = profileOfStorage(session.storagePath);
  session.setPermissionRequestHandler((contents, permission, callback, details) => {
    if (permission === "media" && profileId) {
      requestMedia(profileId, contents, details as Electron.MediaAccessPermissionRequest, callback, note);
      return;
    }
    const allowed = ALLOWED.has(permission);
    note("permission-request", { permission, allowed, origin: details.requestingUrl || contents.getURL() });
    callback(allowed);
  });
  session.setPermissionCheckHandler((contents, permission, requestingOrigin, details) => {
    if (permission === "media" && profileId) {
      const origin = originOf(requestingOrigin);
      const device = details.mediaType === "video" ? { camera: true, microphone: false } : { camera: false, microphone: true };
      return origin !== null && mediaAnswer(siteChoice(profileId, origin), device, contents ? visitingFor(contents, origin) : null) === "grant";
    }
    return ALLOWED.has(permission);
  });
  session.setDevicePermissionHandler(() => false);
  session.setDisplayMediaRequestHandler((_request, callback) => callback({} as never));
  session.on("select-hid-device", (event, _details, callback) => { event.preventDefault(); callback(""); });
  session.on("select-serial-port", (event, _ports, _contents, callback) => { event.preventDefault(); callback(""); });
  session.on("select-usb-device", (event, _details, callback) => { event.preventDefault(); callback(""); });
}
