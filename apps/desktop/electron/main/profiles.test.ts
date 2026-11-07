import assert from "node:assert/strict";
import test from "node:test";

import { isProfilePartition, listedSite, profilePartition, siteAnswer, type SiteAsk, type SiteChoice, siteOnAsking, siteWithChoice } from "./profiles.ts";

test("a profile id maps to its own persisted partition", () => {
  assert.equal(profilePartition("work"), "persist:browser-profile-work");
  assert.throws(() => profilePartition("../up"));
});

test("only a profile's storage counts as a browser profile", () => {
  assert.equal(isProfilePartition("/data/Partitions/browser-profile-work"), true);
  assert.equal(isProfilePartition("/data/Partitions/shell-main"), false);
  assert.equal(isProfilePartition(null), false);
});

const LOCATION: SiteAsk = { camera: false, microphone: false, location: true };
/** As saved before sites could ask for location. */
const OLD: SiteChoice = { camera: "allow", microphone: "block", askedMs: 5 };

test("location goes by its own saved choice, and a site saved before it existed asks", () => {
  assert.equal(siteAnswer(undefined, LOCATION), "ask");
  assert.equal(siteAnswer(OLD, LOCATION), "ask");
  assert.equal(siteAnswer({ ...OLD, location: "allow" }, LOCATION), "grant");
  assert.equal(siteAnswer({ ...OLD, location: "block" }, LOCATION), "deny");
  assert.equal(siteAnswer({ ...OLD, location: "block" }, { camera: true, microphone: false, location: false }), "grant", "the camera isn't location");
  assert.equal(siteAnswer(OLD, { camera: false, microphone: false, location: false }), "deny", "asking for nothing gets nothing");
});

test("a site's entry: asking keeps its choices, and a change touches only what it names", () => {
  assert.deepEqual(siteOnAsking(undefined, 9), { camera: "ask", microphone: "ask", location: "ask", askedMs: 9 });
  assert.deepEqual(siteOnAsking({ ...OLD, location: "block" }, 9), { camera: "allow", microphone: "block", location: "block", askedMs: 9 });
  assert.deepEqual(siteWithChoice(OLD, { location: "allow" }, 9), { camera: "allow", microphone: "block", location: "allow", askedMs: 5 });
  assert.deepEqual(siteWithChoice({ ...OLD, location: "block" }, { camera: null, location: null }, 9), { camera: "allow", microphone: "block", location: "block", askedMs: 5 });
  assert.deepEqual(siteWithChoice(undefined, { camera: "block" }, 9), { camera: "block", microphone: "ask", location: "ask", askedMs: 9 });
});

test("Settings lists a site saved before location with location to ask", () => {
  assert.equal(listedSite("https://a.example", OLD).location, "ask");
  assert.equal(listedSite("https://a.example", { ...OLD, location: "block" }).location, "block");
});
