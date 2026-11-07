// The save offer and autofill, between a guest's preload (guestPasswords.ts) and the app's page, which alone knows
// whether an agent drives a tab (isDriven, src/state/agentPause.ts). A guest's sign-in stays here until its offer is
// answered: the page hears the site and username. A password leaves here only into the guest whose site it was saved
// for, when the page says the tab isn't driven, or to the app's page after the system's check. No agent's call reaches this.
import path from "node:path";
import { fileURLToPath } from "node:url";

import { systemPreferences, webContents, type WebContents } from "electron";

import { handleFromApp } from "./appPage";
import { dev } from "./identity";
import { note } from "./events";
import { profileOfStorage } from "./profiles";
import { loginSite, putLogin, readVault, vaultRefusal, writeVault } from "./browserVault";

export const guestPasswordsPath = path.join(path.dirname(fileURLToPath(import.meta.url)), "guestPasswords.cjs");

/** A sign-in is judged by the next this long: a page without its password box means it worked. */
const ATTEMPT_MS = 30_000;
const OFFER_MS = 120_000;
const NAME_STEP_MS = 5 * 60_000;

type Attempt = { site: string; username: string; password: string; at: number };
type Offer = { guest: number; profile: string; site: string; username: string; password: string; timer: ReturnType<typeof setTimeout> };

const attempts = new Map<number, Attempt>();
const names = new Map<number, { site: string; username: string; at: number }>();
const offers = new Map<string, Offer>();
/** Whether a guest's page shows a password box now: a name-first step gets no password. */
const passwordBoxes = new Map<number, boolean>();

/** Keeps a guest's sign-in for as long as it can still be judged. */
function keepAttempt(guest: number, attempt: Attempt): void {
  attempts.set(guest, attempt);
  setTimeout(() => {
    if (attempts.get(guest) === attempt) attempts.delete(guest);
  }, ATTEMPT_MS);
}

const profileOf = (guest: WebContents) => profileOfStorage(guest.session.storagePath ?? null);
const tellPage = (guest: WebContents, details: object) => guest.hostWebContents?.send("brigadier:guest-login", { from: guest.id, ...details });

function dropOffersOf(guest: number): void {
  for (const [id, offer] of offers) {
    if (offer.guest !== guest) continue;
    clearTimeout(offer.timer);
    offers.delete(id);
  }
}

/** The sign-in worked: the same login only gets its date, and anything else asks, unless the site is on the never list. */
function signedIn(guest: WebContents, attempt: Attempt): void {
  const profile = profileOf(guest);
  if (!profile || vaultRefusal()) return;
  const vault = readVault(profile);
  if (vault.never.includes(attempt.site)) return;
  const known = vault.logins.find((login) => login.site === attempt.site && login.username === attempt.username);
  if (known?.password === attempt.password) {
    known.usedMs = Date.now();
    writeVault(profile, vault);
    return;
  }
  dropOffersOf(guest.id);
  const id = crypto.randomUUID();
  const timer = setTimeout(() => {
    offers.delete(id);
    if (!guest.isDestroyed()) tellPage(guest, { kind: "offer-gone", id });
  }, OFFER_MS);
  offers.set(id, { guest: guest.id, profile, site: attempt.site, username: attempt.username, password: attempt.password, timer });
  note("login-offer", { webContents: guest.id, update: known !== undefined });
  tellPage(guest, { kind: "offer", id, profile, site: attempt.site, username: attempt.username, update: known !== undefined });
}

/** A guest's sign-ins and its fields' news, for each tab. */
export function installGuestLogins(guest: WebContents): void {
  const siteNow = () => loginSite(guest.mainFrame.url);
  let newDocument = false;
  guest.on("did-start-navigation", (details) => {
    if (details.isMainFrame && !details.isSameDocument) newDocument = true;
  });
  guest.ipc.on("brigadier:guest-login-attempt", (event, caught: { username: string; password: string }) => {
    if (event.senderFrame !== guest.mainFrame) return;
    const site = siteNow();
    if (!site || !caught.password) return;
    const step = names.get(guest.id);
    const fromStep = step?.site === site && Date.now() - step.at < NAME_STEP_MS ? step.username : "";
    keepAttempt(guest.id, { site, username: caught.username || fromStep, password: caught.password, at: Date.now() });
    newDocument = false;
  });
  guest.ipc.on("brigadier:guest-login-name", (event, username: string) => {
    const site = siteNow();
    if (event.senderFrame === guest.mainFrame && site) names.set(guest.id, { site, username, at: Date.now() });
  });
  guest.ipc.on("brigadier:guest-login-fields", (event, fields: { hasPassword: boolean; fillable: boolean }) => {
    if (event.senderFrame !== guest.mainFrame) return;
    passwordBoxes.set(guest.id, fields.hasPassword);
    const attempt = attempts.get(guest.id);
    if (attempt) {
      const inTime = Date.now() - attempt.at < ATTEMPT_MS;
      if (!inTime || !fields.hasPassword) {
        attempts.delete(guest.id);
        if (inTime) {
          try {
            signedIn(guest, attempt);
          } catch (error) {
            note("login-offer-failed", { webContents: guest.id, message: (error as Error).message });
          }
        }
      } else if (newDocument) {
        // The page came back with its password box: that sign-in failed.
        attempts.delete(guest.id);
      }
    }
    if (fields.fillable) tellPage(guest, { kind: "fields" });
  });
  guest.once("destroyed", () => {
    attempts.delete(guest.id);
    names.delete(guest.id);
    passwordBoxes.delete(guest.id);
    dropOffersOf(guest.id);
  });
}

/** The page says this tab isn't driven: the site's last used login goes into its boxes. */
function fill(id: number): void {
  const guest = webContents.fromId(id);
  if (!guest || guest.getType() !== "webview") return;
  const profile = profileOf(guest);
  const site = loginSite(guest.mainFrame.url);
  if (!profile || !site || vaultRefusal()) return;
  const logins = readVault(profile).logins.filter((login) => login.site === site);
  const login = logins.toSorted((a, b) => b.usedMs - a.usedMs)[0];
  if (!login) return;
  const step = names.get(id);
  const chosen = (step?.site === site && logins.find((one) => one.username === step.username)) || login;
  note("login-filled", { webContents: id });
  guest.mainFrame.send("brigadier:guest-login-fill", { site, username: chosen.username, password: passwordBoxes.get(id) ? chosen.password : null });
}

function answer(id: string, choice: "save" | "notNow" | "never"): void {
  const offer = offers.get(id);
  if (!offer) return;
  clearTimeout(offer.timer);
  offers.delete(id);
  if (choice === "notNow") return;
  const vault = readVault(offer.profile);
  if (choice === "never") {
    if (!vault.never.includes(offer.site)) vault.never.push(offer.site);
  } else {
    putLogin(vault, offer.site, offer.username, offer.password);
  }
  writeVault(offer.profile, vault);
}

/** Showing a password. macOS asks for Touch ID, whose prompt only the OS draws. Windows and Linux have no check here. */
async function systemCheck(reason: string): Promise<"passed" | "declined" | "unavailable"> {
  // A dev run's scripted check: never in a production build (identity.ts), and the report says which was used.
  const standIn = dev ? process.env.BRIGADIER_DEV_SYSTEM_CHECK : undefined;
  if (standIn) return standIn === "pass" ? "passed" : "declined";
  if (process.platform !== "darwin") return "passed";
  if (!systemPreferences.canPromptTouchID()) return "unavailable";
  return systemPreferences.promptTouchID(reason).then(() => "passed" as const, () => "declined" as const);
}

type Args = { profile: string; id: number; offer: string; answer: "save" | "notNow" | "never"; guest: number; site: string; username: string; password: string };

const OPS: Record<string, (args: Args) => unknown> = {
  list: ({ profile }) => readVault(profile).logins.map(({ id, site, username }) => ({ id, site, username })),
  add: ({ profile, site, username, password }) => {
    const origin = loginSite(site);
    if (!origin || !password) throw new Error("A password needs a web address and a password.");
    const vault = readVault(profile);
    putLogin(vault, origin, username, password);
    writeVault(profile, vault);
  },
  remove: ({ profile, id }) => {
    const vault = readVault(profile);
    vault.logins = vault.logins.filter((login) => login.id !== id);
    writeVault(profile, vault);
  },
  reveal: async ({ profile, id }) => {
    const login = readVault(profile).logins.find((one) => one.id === id);
    if (!login) throw new Error("That password is gone.");
    const checked = await systemCheck(`show your password for ${new URL(login.site).host}`);
    return checked === "passed" ? { kind: "shown", password: login.password } : { kind: checked };
  },
  never: ({ profile }) => readVault(profile).never,
  allow: ({ profile, site }) => {
    const vault = readVault(profile);
    vault.never = vault.never.filter((one) => one !== site);
    writeVault(profile, vault);
  },
  fill: ({ guest }) => fill(guest),
  answer: ({ offer, answer: choice }) => answer(offer, choice),
};

export function installBrowserVault(): void {
  handleFromApp("brigadier:vault", (_event, op: string, args: Args) => {
    const run = OPS[op];
    if (!run) throw new Error(`No password operation ${op}`);
    return run(args);
  });
}
