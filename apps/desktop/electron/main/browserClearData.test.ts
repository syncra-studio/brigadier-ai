import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { fake } from "../testing/fakeElectron.ts";

(globalThis as { BRIGADIER_PRODUCTION?: boolean }).BRIGADIER_PRODUCTION = false;

// After the fake's hooks are in, so this is the fake.
const { session } = await import("electron");
const { installBrowserData, watchProfileSession, watchSiteActivity } = await import("./browserClearData.ts");
const { history, importHistory } = await import("./browserHistory.ts");
const { rendererUrl } = await import("./identity.ts");

fake.userData = fs.mkdtempSync(path.join(os.tmpdir(), "brigadier-clear-test-"));
test.after(() => fs.rmSync(fake.userData, { recursive: true, force: true }));

type Cookie = { domain: string; path: string; name: string; secure: boolean };

/** A profile's session: its cookie jar and its change events, and what Clear asked of it. */
function profileSession(profile: string) {
  const cookies = Object.assign(new EventEmitter(), {
    jar: [] as Cookie[],
    removed: [] as string[],
    get: async () => cookies.jar,
    remove: async (url: string, name: string) => void cookies.removed.push(`${url} ${name}`),
    flushStore: async () => {},
  });
  const cleared: unknown[] = [];
  const created = Object.assign(new EventEmitter(), {
    storagePath: `/data/Partitions/browser-profile-${profile}`,
    cookies,
    cleared,
    clearData: async (options: unknown) => void cleared.push(options),
    clearAuthCache: async () => {},
    flushStorageData: () => {},
  });
  watchProfileSession(created as never);
  return created;
}

const sessions = new Map([["work", profileSession("work")], ["home", profileSession("home")]]);
(session as { fromPartition?: (partition: string) => unknown }).fromPartition = (partition) => sessions.get(partition.replace("persist:browser-profile-", ""));

// Site choices on file before main first reads them.
const choice = (askedMs: number) => ({ camera: "allow", microphone: "ask", askedMs });
fs.writeFileSync(path.join(fake.userData, "browser-sites.json"), JSON.stringify({
  work: { "https://old.test": choice(1000), "https://new.test": choice(5000) },
  home: { "https://new.test": choice(5000) },
}));

installBrowserData();
const fromApp = { sender: { id: 1, getType: () => "window" }, senderFrame: { parent: null, url: rendererUrl } };
const call = (op: string, args: object) => fake.handlers.get("brigadier:browser-data")!(fromApp, op, args) as Promise<unknown>;
const none = { history: false, cookies: false, cache: false, downloads: false, sites: false };

test("Last hours clear only cookies set or overwritten since then, the range's sites' storage, and site choices asked since", async (t) => {
  let now = 1000;
  t.mock.method(Date, "now", () => now);
  const work = sessions.get("work")!;
  const set = (cookie: Cookie, cause = "explicit", removed = false) => work.cookies.emit("changed", {}, cookie, cause, removed);
  const old: Cookie = { domain: ".site.test", path: "/", name: "old", secure: true };
  const fresh: Cookie = { domain: "site.test", path: "/app", name: "fresh", secure: false };
  const overwritten: Cookie = { domain: ".site.test", path: "/", name: "sid", secure: true };
  const gone: Cookie = { domain: "site.test", path: "/", name: "gone", secure: false };
  set(old);
  set(overwritten);
  now = 5000;
  set(fresh);
  // An overwrite comes as a removal then a set: the set dates it.
  set(overwritten, "overwrite", true);
  set(overwritten);
  set(gone);
  set(gone, "explicit", true);
  work.cookies.jar = [old, fresh, overwritten];

  const frame = Object.assign(new EventEmitter(), { session: work });
  watchSiteActivity(frame as never);
  frame.emit("did-frame-navigate", {}, "https://frame.test/page");
  frame.emit("did-frame-navigate", {}, "chrome-error://chromewebdata/");

  await call("clear", { profile: "work", clear: { ...none, sinceMs: 4000, cookies: true, sites: true } });
  assert.deepEqual(work.cookies.removed.sort(), ["http://site.test/app fresh", "https://site.test/ sid"]);
  assert.deepEqual(work.cleared, [{ dataTypes: ["backgroundFetch", "fileSystems", "indexedDB", "localStorage", "serviceWorkers", "webSQL"], origins: ["https://frame.test"] }]);
  assert.deepEqual(sessions.get("home")!.cleared, [], "another profile keeps its data");
  const sites = JSON.parse(fs.readFileSync(path.join(fake.userData, "browser-sites.json"), "utf8")) as Record<string, Record<string, unknown>>;
  assert.deepEqual(Object.keys(sites.work!), ["https://old.test"]);
  assert.deepEqual(Object.keys(sites.home!), ["https://new.test"]);
});

test("All time clears every cookie and site's data; any range clears the whole cache; a tick off keeps its kind", async () => {
  const home = sessions.get("home")!;
  importHistory("home", [{ url: "https://kept.test/", title: "", atMs: 1000 }]);
  await call("clear", { profile: "home", clear: { ...none, sinceMs: 4000, cache: true } });
  assert.deepEqual(home.cleared, [{ dataTypes: ["cache"] }]);
  assert.equal(history("home", "", "all", null, null, 500).length, 1, "history unticked stays");

  home.cleared.length = 0;
  await call("clear", { profile: "home", clear: { ...none, sinceMs: null, cookies: true, history: true } });
  assert.deepEqual(home.cleared, [{ dataTypes: ["cookies", "backgroundFetch", "fileSystems", "indexedDB", "localStorage", "serviceWorkers", "webSQL"] }]);
  assert.equal(history("home", "", "all", null, null, 500).length, 0);
});
