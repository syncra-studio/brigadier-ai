// What a web page may have. The camera, microphone and location go by the profile's saved choice for the site, then
// a question in the tab, then the system's; everything else not listed is refused: no handler at all would mean
// Electron grants it.
import type { Session, WebContents } from "electron";

import { handleFromApp } from "./appPage";
import { locationStatus, openOsSettings, type OsDevice, refusedByOs } from "./browserPermissionsOs";

import type { Note } from "./events";
import { isDeleting, setSiteChoice, siteAsked, siteChoice } from "./profileStore";
import { profileOfStorage, type SiteAsk, siteAnswer } from "./profiles";

const ALLOWED = new Set<string>(["fullscreen", "clipboard-sanitized-write", "pointerLock"]);

type Question = { guest: WebContents; profileId: string; origin: string; asked: SiteAsk; answer: (allowed: boolean) => void };

let nextQuestion = 1;
const questions = new Map<number, Question>();
const watched = new WeakSet<WebContents>();

function originOf(url: string): string | null {
  try {
    const { origin } = new URL(url);
    return origin === "null" ? null : origin;
  } catch {
    return null;
  }
}

/**
 * The system's own question comes only after the person's yes to ours: the first thing they see is ours. What it
 * refuses is told to the tab by name. The page's answer stays the system's: location goes on to Chromium, whose
 * provider gives the page macOS's own refusal.
 */
function grant(guest: WebContents, origin: string, asked: SiteAsk, answer: (allowed: boolean) => void): void {
  refusedByOs(asked).then(
    (refused) => {
      if (refused.length > 0 && !guest.isDestroyed()) guest.hostWebContents?.send("brigadier:guest-permission-refused", { from: guest.id, origin, refused });
      answer(refused.every((device) => device === "location"));
    },
    () => answer(false),
  );
}

function dropQuestions(guest: WebContents, unlessOrigin: string | null): void {
  for (const [request, question] of questions) {
    if (question.guest !== guest || question.origin === unlessOrigin) continue;
    questions.delete(request);
    question.answer(false);
  }
}

/** Leaving the site ends its open question. */
function watchGuest(guest: WebContents): void {
  if (watched.has(guest)) return;
  watched.add(guest);
  guest.on("did-start-navigation", (details) => {
    if (!details.isMainFrame) return;
    dropQuestions(guest, originOf(details.url));
  });
  guest.once("destroyed", () => dropQuestions(guest, null));
}

function request(permission: string, profileId: string, guest: WebContents, requestingUrl: string, asked: SiteAsk, answer: (allowed: boolean) => void, note: Note): void {
  const origin = originOf(requestingUrl);
  const top = originOf(guest.getURL());
  const host = guest.hostWebContents;
  // A question names one site, the tab's: another site's frame asking gets no question, and a page with no tab no door.
  if (!origin || origin !== top || !host || isDeleting(profileId)) {
    note("permission-request", { permission, allowed: false, origin, reason: "not the tab's own site, or the profile is being deleted" });
    answer(false);
    return;
  }
  const decided = siteAnswer(siteChoice(profileId, origin), asked);
  note("permission-request", { permission, profileId, origin, asked, decided, ...(asked.location ? { osLocation: locationStatus() } : {}) });
  if (decided === "grant") {
    grant(guest, origin, asked, answer);
    return;
  }
  if (decided === "deny") {
    answer(false);
    return;
  }
  siteAsked(profileId, origin);
  watchGuest(guest);
  const number = nextQuestion++;
  questions.set(number, { guest, profileId, origin, asked, answer });
  host.send("brigadier:guest-permission", { from: guest.id, request: number, origin, ...asked });
}

export function installPermissionAnswers(): void {
  // "visiting" is the card's Allow while visiting the site: saved for the site in this profile, as "never" is.
  handleFromApp("brigadier:permission-answer", (_event, request: number, choice: "once" | "visiting" | "never" | "dismiss") => {
    const question = questions.get(request);
    if (!question) return;
    questions.delete(request);
    const { guest, profileId, origin, asked, answer } = question;
    const saved = choice === "visiting" ? "allow" : choice === "never" ? "block" : null;
    if (saved) setSiteChoice(profileId, origin, { ...(asked.camera ? { camera: saved } : {}), ...(asked.microphone ? { microphone: saved } : {}), ...(asked.location ? { location: saved } : {}) });
    if (choice === "once" || choice === "visiting") grant(guest, origin, asked, answer);
    else answer(false);
  });
  handleFromApp("brigadier:permission-open-settings", (_event, device: OsDevice) => openOsSettings(device));
}

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
  const profileId = profileOfStorage(session.storagePath);
  session.setPermissionRequestHandler((contents, permission, callback, details) => {
    if (permission === "media" && profileId) {
      const types = (details as Electron.MediaAccessPermissionRequest).mediaTypes ?? [];
      request(permission, profileId, contents, details.requestingUrl, { camera: types.includes("video"), microphone: types.includes("audio"), location: false }, callback, note);
      return;
    }
    if (permission === "geolocation" && profileId) {
      request(permission, profileId, contents, details.requestingUrl, { camera: false, microphone: false, location: true }, callback, note);
      return;
    }
    const allowed = ALLOWED.has(permission);
    note("permission-request", { permission, allowed, origin: details.requestingUrl || contents.getURL() });
    callback(allowed);
  });
  session.setPermissionCheckHandler((_contents, permission, requestingOrigin, details) => {
    if ((permission === "media" || permission === "geolocation") && profileId) {
      const origin = originOf(requestingOrigin);
      const device: SiteAsk = permission === "geolocation"
        ? { camera: false, microphone: false, location: true }
        : { camera: details.mediaType === "video", microphone: details.mediaType !== "video", location: false };
      return origin !== null && siteAnswer(siteChoice(profileId, origin), device) === "grant";
    }
    return ALLOWED.has(permission);
  });
  session.setDevicePermissionHandler(() => false);
  session.setDisplayMediaRequestHandler((_request, callback) => callback({} as never));
  session.on("select-hid-device", (event, _details, callback) => { event.preventDefault(); callback(""); });
  session.on("select-serial-port", (event, _ports, _contents, callback) => { event.preventDefault(); callback(""); });
  session.on("select-usb-device", (event, _details, callback) => { event.preventDefault(); callback(""); });
}
