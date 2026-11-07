import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { fake } from "../testing/fakeElectron.ts";

const { clearVisits, describeGuest, history, importHistory, profileStore, removeStoreOf, removeVisits, watchVisits } = await import("./browserHistory.ts");

fake.userData = fs.mkdtempSync(path.join(os.tmpdir(), "brigadier-history-test-"));
test.after(() => fs.rmSync(fake.userData, { recursive: true, force: true }));

const storeFile = (profile: string) => path.join(fake.userData, "Partitions", `browser-profile-${profile}`, "brigadier-browsing.sqlite");
const urls = (profile: string, query = "") => history(profile, query, "all", null, null, 500).map((visit) => visit.url);
const rows = (profile: string) => profileStore(profile).prepare("SELECT id, url, title, at_ms AS atMs, agent FROM visits ORDER BY id").all() as { id: number; url: string; title: string; atMs: number; agent: number }[];

let nextGuest = 100;
/** A guest of `profile`'s session, as watchVisits sees one. */
function guest(profile: string) {
  const contents = Object.assign(new EventEmitter(), { id: nextGuest++, session: { storagePath: `/data/Partitions/browser-profile-${profile}` } });
  watchVisits(contents as never);
  return {
    id: contents.id,
    go: (url: string) => contents.emit("did-navigate", {}, url),
    title: (title: string) => contents.emit("page-title-updated", {}, title),
  };
}

test("history is newest first, searched by address or title with % _ and \\ taken literally, and filtered by who browsed", () => {
  const profile = "search";
  importHistory(profile, [
    { url: "https://a.test/100%", title: "Percent", atMs: 1000 },
    { url: "https://a.test/x_y", title: "Under", atMs: 2000 },
    { url: "https://a.test/xzy", title: "Plain", atMs: 3000 },
    { url: "https://b.test/back", title: "C:\\path", atMs: 4000 },
  ]);
  assert.deepEqual(urls(profile), ["https://b.test/back", "https://a.test/xzy", "https://a.test/x_y", "https://a.test/100%"]);
  assert.deepEqual(urls(profile, "x_y"), ["https://a.test/x_y"], "_ is no wildcard");
  assert.deepEqual(urls(profile, "100%"), ["https://a.test/100%"]);
  assert.deepEqual(urls(profile, "%"), ["https://a.test/100%"], "% is no wildcard");
  assert.deepEqual(urls(profile, "C:\\"), ["https://b.test/back"], "a title matches, its backslash literal");
  assert.deepEqual(urls(profile, "  plain "), ["https://a.test/xzy"], "trimmed, any case");

  const agentTab = guest(profile);
  describeGuest(agentTab.id, { agent: true, restored: false });
  agentTab.go("https://agent.test/");
  assert.deepEqual(history(profile, "", "agent", null, null, 500).map((visit) => visit.url), ["https://agent.test/"]);
  assert.equal(history(profile, "", "other", null, null, 500).length, 4);
});

test("paging by time and id skips nothing at the same millisecond", () => {
  const profile = "paging";
  importHistory(profile, Array.from({ length: 5 }, (_, index) => ({ url: `https://p.test/${index}`, title: "", atMs: index < 4 ? 5000 : 6000 })));
  const seen: string[] = [];
  let before: { atMs: number; id: number } | null = null;
  for (;;) {
    const page = history(profile, "", "all", before?.atMs ?? null, before?.id ?? null, 2);
    if (page.length === 0) break;
    seen.push(...page.map((visit) => visit.url));
    const last = page[page.length - 1]!;
    before = { atMs: last.atMs, id: last.id };
  }
  assert.deepEqual(seen, ["https://p.test/4", "https://p.test/3", "https://p.test/2", "https://p.test/1", "https://p.test/0"]);
});

test("importHistory adds web pages once, as the user's own", () => {
  const profile = "imported";
  const items = [
    { url: "https://one.test/", title: "One", atMs: 1000 },
    { url: "http://two.test/", title: "", atMs: 2000 },
    { url: "file:///etc/hosts", title: "Hosts", atMs: 3000 },
    { url: "chrome://settings", title: "Settings", atMs: 4000 },
    { url: "about:blank", title: "", atMs: 5000 },
  ];
  assert.equal(importHistory(profile, items), 2);
  assert.equal(importHistory(profile, items), 0, "importing twice adds nothing");
  assert.equal(importHistory(profile, [{ url: "https://one.test/", title: "One", atMs: 9000 }]), 1, "the same page at another time is another visit");
  assert.ok(rows(profile).every((row) => row.agent === 0));
});

test("clearing by range keeps what came before it; all time keeps nothing", () => {
  const profile = "ranged";
  importHistory(profile, [1000, 2000, 3000].map((atMs) => ({ url: `https://r.test/${atMs}`, title: "", atMs })));
  clearVisits(profile, 2000);
  assert.deepEqual(urls(profile), ["https://r.test/1000"]);
  clearVisits(profile, null);
  assert.deepEqual(urls(profile), []);
});

test("a reload is one visit that moves up, and an agent's tab marks its visit", (t) => {
  const profile = "reload";
  let now = 1000;
  t.mock.method(Date, "now", () => now);
  const tab = guest(profile);
  tab.go("https://site.test/");
  tab.title("Site");
  now = 2000;
  tab.go("https://site.test/");
  assert.deepEqual(rows(profile).map(({ url, title, atMs }) => ({ url, title, atMs })), [{ url: "https://site.test/", title: "Site", atMs: 2000 }]);
  describeGuest(tab.id, { agent: true, restored: false });
  assert.equal(rows(profile)[0]!.agent, 1, "the page it already had counts as the agent's");
  tab.go("about:blank");
  assert.equal(rows(profile).length, 1, "not a web page, not a visit");
});

test("a tab restored at restart adds no visit, whether told before or after its first load", () => {
  const profile = "restored";
  importHistory(profile, [{ url: "https://kept.test/", title: "Kept", atMs: 1000 }]);
  const [old] = rows(profile);

  // Told first: the load carries on the newest row of that address.
  const before = guest(profile);
  describeGuest(before.id, { agent: false, restored: true });
  before.go("https://kept.test/");
  assert.deepEqual(rows(profile).map((row) => row.id), [old!.id]);
  before.title("Kept again");
  assert.equal(rows(profile)[0]!.title, "Kept again", "its title lands on the row it carried on");

  // Told after: the visit its load made goes, and the tab carries on the older row.
  const after = guest(profile);
  after.go("https://kept.test/");
  assert.equal(rows(profile).length, 2);
  describeGuest(after.id, { agent: false, restored: true });
  assert.deepEqual(rows(profile).map((row) => row.id), [old!.id]);
  after.title("After");
  assert.equal(rows(profile)[0]!.title, "After");

  // Restored at a page with no row: it carries on nothing, and its reload doesn't bring one.
  const gone = guest(profile);
  describeGuest(gone.id, { agent: false, restored: true });
  gone.go("https://never.test/");
  gone.go("https://never.test/");
  assert.deepEqual(urls(profile), ["https://kept.test/"]);
  gone.go("https://next.test/");
  assert.deepEqual(urls(profile).sort(), ["https://kept.test/", "https://next.test/"], "the next page is a visit");
});

test("a removed visit stays removed while its tab reloads, and the tab's next page is a visit", () => {
  const profile = "removed";
  const tab = guest(profile);
  tab.go("https://gone.test/");
  removeVisits(profile, rows(profile).map((row) => row.id));
  tab.go("https://gone.test/");
  tab.title("Gone");
  assert.deepEqual(rows(profile), []);
  tab.go("https://other.test/");
  assert.deepEqual(urls(profile), ["https://other.test/"]);
  clearVisits(profile, null);
  tab.go("https://other.test/");
  assert.deepEqual(urls(profile), [], "cleared all time, the same guard holds");
});

test("a deleted profile's store stays gone while its session's last events arrive", () => {
  const profile = "deleted";
  const tab = guest(profile);
  tab.go("https://before.test/");
  assert.ok(fs.existsSync(storeFile(profile)));
  removeStoreOf(profile);
  assert.equal(fs.existsSync(storeFile(profile)), false);
  tab.title("Late title");
  tab.go("https://after.test/");
  importHistory(profile, [{ url: "https://late.test/", title: "", atMs: 1 }]);
  assert.equal(fs.existsSync(storeFile(profile)), false, "nothing writes the file again");
  assert.equal(fs.existsSync(`${storeFile(profile)}-wal`), false);
});
