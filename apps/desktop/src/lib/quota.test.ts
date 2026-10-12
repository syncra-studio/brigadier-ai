import assert from "node:assert/strict";
import { test } from "node:test";

import type { QuotaSnapshot, QuotaWindow } from "@/ipc/generated";
import {
  dailyPeaks,
  drawsDailyPeak,
  glance,
  left,
  leftTone,
  percentLeft,
  usageLevel,
  usageTone,
  windowTitle,
} from "@/lib/quota";

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

// Like the agents' own CLIs: usage takes the warning colour from 80% used and the danger colour
// from 90%, by the share used alone; 32% or 69% look normal.
test("usage is normal below 80% used, a warning from 80% and danger from 90%", () => {
  assert.equal(usageLevel(0), "normal");
  assert.equal(usageLevel(32), "normal");
  assert.equal(usageLevel(69), "normal");
  assert.equal(usageLevel(79.9), "normal");
  assert.equal(usageLevel(80), "warning");
  assert.equal(usageLevel(89), "warning");
  assert.equal(usageLevel(90), "danger");
  assert.equal(usageLevel(100), "danger");
});

test("a reached limit is danger whatever the share used", () => {
  assert.equal(usageLevel(12, true), "danger");
  assert.equal(usageTone(12, { limitReached: true }).text, "text-destructive");
});

test("a place keeps its own normal look, and shares the warning and danger ones", () => {
  const normal = { text: "text-muted-foreground", fill: "bg-primary" };
  assert.deepEqual(
    [32, 85, 95].map((used) => usageTone(used, { normal }).fill),
    ["bg-primary", "bg-warning", "bg-destructive"],
  );
  // Shown as "% left": 15% left is 85% used.
  assert.equal(usageTone(100 - 15).level, "warning");
});

test("what is left shows amber at 20% or less and red at 10% or less", () => {
  assert.deepEqual(
    [21, 20, 11, 10, 0].map((share) => leftTone(share).level),
    ["normal", "warning", "warning", "danger", "danger"],
  );
  assert.equal(leftTone(80, { limitReached: true }).level, "danger");
  assert.equal(left(window("w", 82, null)), 18);
});

test("a window's title folds in what it is limited to", () => {
  assert.equal(windowTitle({ ...window("w", 10, null), label: "Weekly (Fable)" }), "Weekly · Fable");
  assert.equal(windowTitle({ ...window("w", 10, null), label: "Weekly", bucket: "gpt-6-luna" }), "Weekly · gpt-6-luna");
  assert.equal(windowTitle({ ...window("w", 10, null), label: "5-hour" }), "5-hour");
});

const day = (offset: number, hour: number) => new Date(2026, 9, 1 + offset, hour).getTime();

test("over 30 days a window shorter than a day draws each day's peak, held across the day", () => {
  assert.equal(drawsDailyPeak(window("five_hour", 0, 300), 30), true);
  assert.equal(drawsDailyPeak(window("five_hour", 0, 300), 7), false);
  assert.equal(drawsDailyPeak(window("seven_day", 0, 10_080), 30), false);
  assert.equal(drawsDailyPeak(window("other", 0, null), 30), false);

  const start = day(0, 12);
  const end = day(2, 9);
  const samples = [
    { atMs: day(0, 6), usedPercent: 99 }, // before the range
    { atMs: day(0, 13), usedPercent: 40 },
    { atMs: day(0, 18), usedPercent: 85 },
    { atMs: day(0, 23), usedPercent: 5 },
    { atMs: day(2, 8), usedPercent: 30 }, // no reading on the 2nd
  ];
  assert.deepEqual(dailyPeaks(samples, start, end), [
    { atMs: start, usedPercent: 85 },
    { atMs: day(1, 0) - 1, usedPercent: 85 },
    { atMs: day(2, 0), usedPercent: 30 },
    { atMs: end, usedPercent: 30 },
  ]);
});
