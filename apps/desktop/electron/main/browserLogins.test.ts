import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test, { type TestContext } from "node:test";

import { fake } from "../testing/fakeElectron.ts";

// A dev run, where the scripted check may stand in for Touch ID. Production has its own file, since `dev` is read once.
(globalThis as { BRIGADIER_PRODUCTION?: boolean }).BRIGADIER_PRODUCTION = false;
process.env.BRIGADIER_DEV = "1";
delete process.env.BRIGADIER_PAGE_DIR;
delete process.env.BRIGADIER_RENDERER_URL;
const { installBrowserVault, installGuestLogins } = await import("./browserLogins.ts");
const { readVault } = await import("./browserVault.ts");
const { rendererUrl } = await import("./identity.ts");

fake.userData = fs.mkdtempSync(path.join(os.tmpdir(), "brigadier-logins-test-"));
test.after(() => fs.rmSync(fake.userData, { recursive: true, force: true }));
installBrowserVault();

const SITE = "https://site.test";
const ATTEMPT_MS = 30_000;
const OFFER_MS = 120_000;

type Told = { from: number; kind: string; id?: string; profile?: string; site?: string; username?: string; update?: boolean };
type Filled = { site: string; username: string; password: string | null };

let nextGuest = 1000;

/** A tab's page, as main sees it: what its preload sends comes in here, and what main answers is kept. */
function guestOn(profile: string, url = `${SITE}/login`) {
  const told: Told[] = [];
  const filled: Filled[] = [];
  const ipc = new Map<string, (event: unknown, payload: unknown) => void>();
  const navigation: ((details: { isMainFrame: boolean; isSameDocument: boolean }) => void)[] = [];
  const mainFrame = { url, send: (_channel: string, fill: Filled) => filled.push(fill) };
  const guest = {
    id: nextGuest++,
    mainFrame,
    session: { storagePath: `/data/Partitions/browser-profile-${profile}` },
    hostWebContents: { send: (_channel: string, details: Told) => told.push(details) },
    isDestroyed: () => false,
    getType: () => "webview",
    on: (_event: string, listener: (typeof navigation)[number]) => navigation.push(listener),
    once: () => {},
    ipc: { on: (channel: string, listener: (event: unknown, payload: unknown) => void) => ipc.set(channel, listener) },
  };
  fake.contents.set(guest.id, guest);
  installGuestLogins(guest as never);
  const send = (channel: string, payload: unknown, frame: unknown = mainFrame) => ipc.get(`brigadier:guest-login-${channel}`)!({ senderFrame: frame }, payload);
  return {
    id: guest.id,
    told,
    filled,
    frame: mainFrame,
    send,
    attempt: (username: string, password: string) => send("attempt", { username, password }),
    name: (username: string) => send("name", username),
    fields: (hasPassword: boolean, fillable = hasPassword) => send("fields", { hasPassword, fillable }),
    navigated: () => navigation.forEach((listener) => listener({ isMainFrame: true, isSameDocument: false })),
    offers: () => told.filter(({ kind }) => kind === "offer"),
  };
}

/** A call from the app's own page. */
const vault = (op: string, args: object) =>
  fake.handlers.get("brigadier:vault")!({ sender: { id: 1, getType: () => "window" }, senderFrame: { parent: null, url: rendererUrl } }, op, args);

function saveLogin(profile: string, username: string, password: string, site = SITE): void {
  vault("add", { profile, site, username, password });
}

const clock = (t: TestContext) => t.mock.timers.enable({ apis: ["setTimeout", "Date"], now: 1_000_000 });

test("the password box going away means the sign-in worked: an offer with no password", () => {
  const page = guestOn("gone");
  page.fields(true);
  page.attempt("fake-sam", "fake-Tulip-41");
  assert.deepEqual(page.offers(), []);
  page.fields(false);
  const [offer] = page.offers();
  assert.deepEqual({ ...offer, id: undefined }, { from: page.id, kind: "offer", id: undefined, profile: "gone", site: SITE, username: "fake-sam", update: false });
  assert.equal(JSON.stringify(page.told).includes("fake-Tulip-41"), false, "the page never hears the password");
  assert.deepEqual(readVault("gone").logins, [], "nothing is saved before an answer");
});

test("a sign-in that comes back to a page with its password box failed", () => {
  const page = guestOn("failed");
  page.fields(true);
  page.attempt("fake-sam", "wrong");
  page.navigated();
  page.fields(true, true);
  // Whatever happens next, that attempt is over.
  page.fields(false);
  assert.deepEqual(page.offers(), []);
});

test("the box still showing on the same document isn't a judgment yet", () => {
  const page = guestOn("same-page");
  page.fields(true);
  page.attempt("fake-sam", "fake-Tulip-41");
  page.fields(true, false);
  assert.deepEqual(page.offers(), []);
  page.fields(false);
  assert.equal(page.offers().length, 1);
});

test("a sign-in not judged within 30 seconds is forgotten", (t) => {
  clock(t);
  const page = guestOn("timeout");
  page.fields(true);
  page.attempt("fake-sam", "fake-Tulip-41");
  t.mock.timers.tick(ATTEMPT_MS);
  page.fields(false);
  assert.deepEqual(page.offers(), []);
});

test("just inside the 30 seconds it still counts", (t) => {
  clock(t);
  const page = guestOn("in-time");
  page.fields(true);
  page.attempt("fake-sam", "fake-Tulip-41");
  t.mock.timers.tick(ATTEMPT_MS - 1);
  page.fields(false);
  assert.equal(page.offers().length, 1);
});

test("an attempt with no password, or from a subframe, or off the web, isn't one", () => {
  const empty = guestOn("not-attempts");
  empty.attempt("fake-sam", "");
  empty.fields(false);
  assert.deepEqual(empty.offers(), []);

  const framed = guestOn("not-attempts");
  framed.send("attempt", { username: "fake-sam", password: "fake-one" }, { other: "frame" });
  framed.fields(false);
  assert.deepEqual(framed.offers(), []);

  const offline = guestOn("not-attempts", "file:///tmp/login.html");
  offline.attempt("fake-sam", "fake-one");
  offline.fields(false);
  assert.deepEqual(offline.offers(), []);
});

test("a username from a name-first step goes with the password of the next", () => {
  const page = guestOn("name-step");
  page.name("fake-sam");
  page.fields(true);
  page.attempt("", "fake-Tulip-41");
  page.fields(false);
  assert.equal(page.offers()[0]!.username, "fake-sam");
});

test("the same login signing in gets no offer, only its date", (t) => {
  clock(t);
  const profile = "same-login";
  saveLogin(profile, "fake-sam", "fake-Tulip-41");
  const usedBefore = readVault(profile).logins[0]!.usedMs;
  t.mock.timers.tick(5000);
  const page = guestOn(profile);
  page.fields(true);
  page.attempt("fake-sam", "fake-Tulip-41");
  page.fields(false);
  assert.deepEqual(page.offers(), []);
  assert.equal(readVault(profile).logins[0]!.usedMs, usedBefore + 5000);
});

test("a known username with a new password offers an update, and Save takes it", () => {
  const profile = "update";
  saveLogin(profile, "fake-sam", "fake-old");
  const page = guestOn(profile);
  page.fields(true);
  page.attempt("fake-sam", "fake-new");
  page.fields(false);
  const [offer] = page.offers();
  assert.equal(offer!.update, true);
  vault("answer", { offer: offer!.id, answer: "save" });
  const { logins } = readVault(profile);
  assert.equal(logins.length, 1);
  assert.equal(logins[0]!.password, "fake-new");
});

test("Not now saves nothing, the offer's answered once, and the next sign-in asks again", () => {
  const profile = "not-now";
  const page = guestOn(profile);
  page.fields(true);
  page.attempt("fake-sam", "fake-one");
  page.fields(false);
  const [first] = page.offers();
  vault("answer", { offer: first!.id, answer: "notNow" });
  vault("answer", { offer: first!.id, answer: "save" });
  assert.deepEqual(readVault(profile).logins, [], "what was held went with the first answer");
  page.fields(true);
  page.attempt("fake-sam", "fake-one");
  page.fields(false);
  assert.equal(page.offers().length, 2);
});

test("an unanswered offer goes after 120 seconds, with its password", (t) => {
  clock(t);
  const profile = "offer-gone";
  const page = guestOn(profile);
  page.fields(true);
  page.attempt("fake-sam", "fake-one");
  page.fields(false);
  const [offer] = page.offers();
  t.mock.timers.tick(OFFER_MS);
  assert.deepEqual(page.told.at(-1), { from: page.id, kind: "offer-gone", id: offer!.id });
  vault("answer", { offer: offer!.id, answer: "save" });
  assert.deepEqual(readVault(profile).logins, []);
});

test("the never list is per profile", () => {
  const work = guestOn("never-work");
  work.fields(true);
  work.attempt("fake-sam", "fake-one");
  work.fields(false);
  vault("answer", { offer: work.offers()[0]!.id, answer: "never" });
  assert.deepEqual(vault("never", { profile: "never-work" }), [SITE]);
  assert.deepEqual(readVault("never-work").logins, []);

  work.fields(true);
  work.attempt("fake-sam", "fake-one");
  work.fields(false);
  assert.equal(work.offers().length, 1, "that profile isn't asked again");

  const personal = guestOn("never-personal");
  assert.deepEqual(vault("never", { profile: "never-personal" }), []);
  personal.fields(true);
  personal.attempt("fake-sam", "fake-one");
  personal.fields(false);
  assert.equal(personal.offers().length, 1, "another profile still is");

  vault("allow", { profile: "never-work", site: SITE });
  work.fields(true);
  work.attempt("fake-sam", "fake-one");
  work.fields(false);
  assert.equal(work.offers().length, 2, "Offer again asks again");
});

test("a list has sites and usernames, never a password", () => {
  const profile = "listed";
  saveLogin(profile, "fake-sam", "fake-Tulip-41");
  assert.deepEqual(vault("list", { profile }), [{ id: 1, site: SITE, username: "fake-sam" }]);
});

test("only the app's page reaches the vault", () => {
  assert.throws(
    () => fake.handlers.get("brigadier:vault")!({ sender: { id: 2, getType: () => "webview" }, senderFrame: { parent: null, url: `${SITE}/` } }, "list", { profile: "listed" }),
    /for the app's own page/,
  );
});

test("a page's boxes are told to the app, which asks to fill: the site's last used login", (t) => {
  clock(t);
  const profile = "fill";
  saveLogin(profile, "fake-old", "fake-old-password");
  t.mock.timers.tick(1000);
  saveLogin(profile, "fake-recent", "fake-recent-password");
  saveLogin(profile, "fake-other-site", "fake-other", "https://other.test");

  const page = guestOn(profile);
  page.fields(true);
  assert.deepEqual(page.told.at(-1), { from: page.id, kind: "fields" }, "the app hears of the boxes, and no value");
  assert.deepEqual(page.filled, [], "main fills only when the app says the tab isn't driven");
  vault("fill", { guest: page.id });
  assert.deepEqual(page.filled, [{ site: SITE, username: "fake-recent", password: "fake-recent-password" }]);
});

test("a name-first step gets the name, and no password until its box shows", () => {
  const profile = "fill-step";
  saveLogin(profile, "fake-sam", "fake-Tulip-41");
  saveLogin(profile, "fake-kim", "fake-Comet-19");

  const page = guestOn(profile);
  page.fields(false, true);
  vault("fill", { guest: page.id });
  assert.equal(page.filled.at(-1)!.password, null);

  // The login whose name the user went on with, even when it isn't the last used.
  page.name("fake-sam");
  page.fields(true);
  vault("fill", { guest: page.id });
  assert.deepEqual(page.filled.at(-1), { site: SITE, username: "fake-sam", password: "fake-Tulip-41" });
});

test("fill names the site the page is on now, and gives nothing for a site with no login", () => {
  const profile = "fill-site";
  saveLogin(profile, "fake-sam", "fake-Tulip-41");
  const page = guestOn(profile);
  page.fields(true);
  page.frame.url = "https://moved-on.test/";
  vault("fill", { guest: page.id });
  assert.deepEqual(page.filled, []);
  page.frame.url = "file:///tmp/x.html";
  vault("fill", { guest: page.id });
  assert.deepEqual(page.filled, []);
  page.frame.url = `${SITE}/somewhere/else`;
  vault("fill", { guest: page.id });
  assert.equal(page.filled[0]!.site, SITE);
});

test("fill goes only to a tab's page", () => {
  const profile = "fill-webview";
  saveLogin(profile, "fake-sam", "fake-Tulip-41");
  const sent: unknown[] = [];
  fake.contents.set(5, { getType: () => "window", session: { storagePath: `/data/Partitions/browser-profile-${profile}` }, mainFrame: { url: `${SITE}/`, send: (...args: unknown[]) => sent.push(args) } });
  vault("fill", { guest: 5 });
  assert.deepEqual(sent, []);
});

test("a dev run's stand-in answers for the system's check", async (t) => {
  const profile = "stand-in";
  saveLogin(profile, "fake-sam", "fake-Tulip-41");
  t.after(() => { delete process.env.BRIGADIER_DEV_SYSTEM_CHECK; });
  const id = readVault(profile).logins[0]!.id;
  process.env.BRIGADIER_DEV_SYSTEM_CHECK = "pass";
  assert.deepEqual(await vault("reveal", { profile, id }), { kind: "shown", password: "fake-Tulip-41" });
  process.env.BRIGADIER_DEV_SYSTEM_CHECK = "fail";
  assert.deepEqual(await vault("reveal", { profile, id }), { kind: "declined" });
  assert.deepEqual(fake.touchIdAsked, []);
});
