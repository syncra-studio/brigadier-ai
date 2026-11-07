// What a web page may have. The camera, microphone and location go by the profile's saved choice for the site, then
// a question in the tab, then the system's; everything else not listed is refused: no handler at all would mean
// Electron grants it.
import type { Session, WebContents } from "electron";

import { handleFromApp } from "./appPage";
import { openOsSettings, type OsDevice, osStatus, refusedByOs } from "./browserPermissionsOs";
import { answerFromOs, cardAllows, type CardAnswer, checkAllows, decideRequest, savedFromCard } from "./browserPermissionsRules";

import type { Note } from "./events";
import { isDeleting, setSiteChoice, siteAsked, siteChoice } from "./profileStore";
import { profileOfStorage, type SiteAsk } from "./profiles";

const ALLOWED = new Set<string>(["fullscreen", "clipboard-sanitized-write", "pointerLock"]);

type Question = { guest: WebContents; profileId: string; origin: string; asked: SiteAsk; answer: (allowed: boolean) => void; note: Note };

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

/** The system's own question comes only after the person's yes to ours: the first thing they see is ours. */
function grant(guest: WebContents, origin: string, asked: SiteAsk, answer: (allowed: boolean) => void, note: Note): void {
  void answerFromOs({
    refusedByOs: () => refusedByOs(asked),
    log: (refused) => note("permission-os", { origin, asked, refused, os: osStatus(asked) }),
    tell: (refused) => {
      if (!guest.isDestroyed()) guest.hostWebContents?.send("brigadier:guest-permission-refused", { from: guest.id, origin, refused });
    },
    answer,
  });
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
  const host = guest.hostWebContents;
  const site = origin ? siteChoice(profileId, origin) : undefined;
  const decided = decideRequest({ origin, top: originOf(guest.getURL()), hasTab: !!host, deleting: isDeleting(profileId), asked, site, platform: process.platform });
  if (decided === "not-the-tabs-site" || !origin || !host) {
    note("permission-request", { permission, allowed: false, origin, reason: "not the tab's own site, or the profile is being deleted" });
    answer(false);
    return;
  }
  if (decided === "no-location") {
    note("permission-request", { permission, profileId, origin, allowed: false, reason: "no location on this system" });
    host.send("brigadier:guest-permission-refused", { from: guest.id, origin, refused: ["location"] });
    answer(false);
    return;
  }
  note("permission-request", { permission, profileId, origin, asked, decided, os: osStatus(asked) });
  if (decided === "grant") {
    grant(guest, origin, asked, answer, note);
    return;
  }
  if (decided === "deny") {
    answer(false);
    return;
  }
  siteAsked(profileId, origin);
  watchGuest(guest);
  const number = nextQuestion++;
  questions.set(number, { guest, profileId, origin, asked, answer, note });
  host.send("brigadier:guest-permission", { from: guest.id, request: number, origin, ...asked });
}

export function installPermissionAnswers(): void {
  handleFromApp("brigadier:permission-answer", (_event, request: number, choice: CardAnswer) => {
    const question = questions.get(request);
    if (!question) return;
    questions.delete(request);
    const { guest, profileId, origin, asked, answer, note } = question;
    const saved = savedFromCard(choice, asked);
    if (Object.keys(saved).length > 0) setSiteChoice(profileId, origin, saved);
    if (cardAllows(choice)) grant(guest, origin, asked, answer, note);
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
      return checkAllows(origin, device, origin ? siteChoice(profileId, origin) : undefined, process.platform);
    }
    return ALLOWED.has(permission);
  });
  session.setDevicePermissionHandler(() => false);
  session.setDisplayMediaRequestHandler((_request, callback) => callback({} as never));
  session.on("select-hid-device", (event, _details, callback) => { event.preventDefault(); callback(""); });
  session.on("select-serial-port", (event, _ports, _contents, callback) => { event.preventDefault(); callback(""); });
  session.on("select-usb-device", (event, _details, callback) => { event.preventDefault(); callback(""); });
}
