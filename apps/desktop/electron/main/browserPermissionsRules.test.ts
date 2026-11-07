import assert from "node:assert/strict";
import test, { type TestContext } from "node:test";

import { answerFromOs, cardAllows, checkAllows, decideRequest, locationAllowed, type LocationStatus, savedFromCard } from "./browserPermissionsRules.ts";
import type { SiteAsk, SiteChoice, SitePermission } from "./profiles.ts";

const LOCATION: SiteAsk = { camera: false, microphone: false, location: true };
const CAMERA: SiteAsk = { camera: true, microphone: false, location: false };
const BOTH: SiteAsk = { camera: true, microphone: true, location: false };
const SITE = "https://maps.example";
const saved = (location: SitePermission): SiteChoice => ({ camera: "ask", microphone: "ask", location, askedMs: 1 });
const asking = { origin: SITE, top: SITE, hasTab: true, deleting: false, asked: LOCATION, site: undefined, platform: "darwin" as const };

test("only the tab's own site gets a question, and not while its profile is being deleted", () => {
  assert.equal(decideRequest(asking), "ask");
  assert.equal(decideRequest({ ...asking, origin: "https://frame.example" }), "not-the-tabs-site");
  assert.equal(decideRequest({ ...asking, origin: null }), "not-the-tabs-site");
  assert.equal(decideRequest({ ...asking, hasTab: false }), "not-the-tabs-site");
  assert.equal(decideRequest({ ...asking, deleting: true }), "not-the-tabs-site");
});

test("a saved choice answers location with no card", () => {
  assert.equal(decideRequest({ ...asking, site: saved("allow") }), "grant");
  assert.equal(decideRequest({ ...asking, site: saved("block") }), "deny");
  assert.equal(decideRequest({ ...asking, site: saved("ask") }), "ask");
});

test("Linux refuses location before any card, whatever is saved, and leaves the camera to ask", () => {
  const linux = { ...asking, platform: "linux" as const };
  assert.equal(decideRequest(linux), "no-location");
  assert.equal(decideRequest({ ...linux, site: saved("allow") }), "no-location");
  assert.equal(decideRequest({ ...linux, asked: CAMERA }), "ask");
  assert.equal(decideRequest({ ...asking, platform: "win32" }), "ask");
});

test("a page's check reads only a saved Allow, and never location on Linux", () => {
  assert.equal(checkAllows(SITE, LOCATION, saved("allow"), "darwin"), true);
  assert.equal(checkAllows(SITE, LOCATION, saved("ask"), "darwin"), false);
  assert.equal(checkAllows(SITE, LOCATION, saved("block"), "darwin"), false);
  assert.equal(checkAllows(null, LOCATION, saved("allow"), "darwin"), false);
  assert.equal(checkAllows(SITE, LOCATION, saved("allow"), "linux"), false);
});

test("the card's answers: what is saved for what was asked, and what lets the system go on", () => {
  assert.deepEqual(savedFromCard("visiting", LOCATION), { location: "allow" });
  assert.deepEqual(savedFromCard("never", LOCATION), { location: "block" });
  assert.deepEqual(savedFromCard("visiting", BOTH), { camera: "allow", microphone: "allow" });
  assert.deepEqual(savedFromCard("never", CAMERA), { camera: "block" });
  assert.deepEqual(savedFromCard("once", LOCATION), {});
  assert.deepEqual(savedFromCard("dismiss", LOCATION), {});
  assert.deepEqual((["once", "visiting", "never", "dismiss"] as const).map(cardAllows), [true, true, false, false]);
});

for (const [status, allowed] of [["allowed", true], ["denied", false], ["restricted", false], ["off", false], ["unknown", false]] as const) {
  test(`macOS already says ${status}: no question, and the page is ${allowed ? "let through" : "refused"}`, async () => {
    let asks = 0;
    assert.equal(await locationAllowed(() => status, () => { asks++; }), allowed);
    assert.equal(asks, 0);
  });
}

const flush = async () => { for (let i = 0; i < 5; i++) await Promise.resolve(); };

for (const [answer, allowed] of [["allowed", true], ["denied", false]] as const) {
  test(`no answer yet: macOS is asked once, and its ${answer} decides`, async (t: TestContext) => {
    t.mock.timers.enable({ apis: ["setTimeout", "Date"], now: 0 });
    let status: LocationStatus = "undetermined";
    let asks = 0;
    const waiting = locationAllowed(() => status, () => { asks++; }, { answerMs: 1000, readMs: 100 });
    t.mock.timers.tick(100);
    await flush();
    status = answer;
    t.mock.timers.tick(100);
    assert.equal(await waiting, allowed);
    assert.equal(asks, 1);
  });
}

test("nobody answers macOS: the limit counts as no", async (t: TestContext) => {
  t.mock.timers.enable({ apis: ["setTimeout", "Date"], now: 0 });
  let settled: boolean | null = null;
  const waiting = locationAllowed(() => "undetermined", () => {}, { answerMs: 300, readMs: 100 }).then((allowed) => { settled = allowed; });
  for (let i = 0; i < 2; i++) {
    t.mock.timers.tick(100);
    await flush();
  }
  assert.equal(settled, null, "still waiting inside the limit");
  t.mock.timers.tick(100);
  await waiting;
  assert.equal(settled, false);
});

test("the system's answer: logged, a refusal told by name, and the page answered once", async () => {
  const run = async (refusedByOs: () => Promise<string[]>) => {
    const seen: unknown[] = [];
    await answerFromOs({ refusedByOs, log: (refused) => seen.push(["log", refused]), tell: (refused) => seen.push(["tell", refused]), answer: (allowed) => seen.push(["answer", allowed]) });
    return seen;
  };
  assert.deepEqual(await run(async () => []), [["log", []], ["answer", true]]);
  assert.deepEqual(await run(async () => ["location"]), [["log", ["location"]], ["tell", ["location"]], ["answer", false]]);
  assert.deepEqual(await run(async () => { throw new Error("no"); }), [["answer", false]]);
});
