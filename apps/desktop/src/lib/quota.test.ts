import assert from "node:assert/strict";
import { test } from "node:test";

import type { QuotaSnapshot, QuotaWindow } from "@/ipc/generated";
import { glance, percentLeft } from "@/lib/quota";

function window(id: string, usedPercent: number, windowMinutes: number | null): QuotaWindow {
  return { id, label: id, usedPercent, resetsAtMs: null, windowMinutes, bucket: null, model: null };
}

function snapshot(windows: QuotaWindow[]): QuotaSnapshot {
  return { provider: "claude", windows, limit: null, observedAtMs: 0, source: "read" };
}

test("an account has as much left as its fullest window allows", () => {
  const quota = snapshot([window("seven_day", 70, 10_080), window("five_hour", 20, 300)]);
  assert.equal(percentLeft(quota), 30);
  assert.deepEqual(
    glance(quota).map((shown) => shown.id),
    ["five_hour", "seven_day"],
  );
});

test("an account whose usage was never read has nothing to show", () => {
  assert.equal(percentLeft(null), null);
  assert.equal(percentLeft(snapshot([])), null);
});
