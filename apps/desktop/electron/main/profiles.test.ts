import assert from "node:assert/strict";
import test from "node:test";

import { isProfilePartition, profilePartition } from "./profiles.ts";

test("a profile id maps to its own persisted partition", () => {
  assert.equal(profilePartition("work"), "persist:browser-profile-work");
  assert.throws(() => profilePartition("../up"));
});

test("only a profile's storage counts as a browser profile", () => {
  assert.equal(isProfilePartition("/data/Partitions/browser-profile-work"), true);
  assert.equal(isProfilePartition("/data/Partitions/shell-main"), false);
  assert.equal(isProfilePartition(null), false);
});
