import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

type Rendering = {
  timelines: number; threadText: string; closedSteps: number; line: string; steps: number; details: number;
  opened: string; image: boolean; left: string; current: string; home: string; homeShot: string; end: string;
  playing: { step: string; button: string }; played: { step: string; button: string }; reads: string[];
};

// The real worker thread in Chromium: its computer calls fold into one timeline, which the keys
// and Play step through, reading each batch's screenshot.
test("a worker's computer calls fold into one timeline that keys and Play step through", { timeout: 60000 }, async (t) => {
  const rendering = JSON.parse(await renderFixturePage(t, "computer-timeline.html", "computer-timeline-result", 15000)) as Rendering;
  assert.equal(rendering.timelines, 1);
  // The other tools keep their rows; the computer calls have none besides the timeline.
  assert.match(rendering.threadText, /Read a file/);
  assert.doesNotMatch(rendering.threadText, /used (apps|observe|act)/i);
  assert.equal(rendering.closedSteps, 0);
  assert.equal(rendering.line, "Used the computer · 9 steps in target-range");
  assert.equal(rendering.steps, 9);
  assert.equal(rendering.details, 1);
  assert.equal(rendering.opened, "Step 8 of 8");
  assert.ok(rendering.image);
  assert.equal(rendering.left, "Step 7 of 8");
  assert.match(rendering.current, /Selected text in “Notes”/);
  assert.equal(rendering.home, "Step 1 of 8");
  assert.equal(rendering.homeShot, "image");
  assert.equal(rendering.end, "Step 8 of 8");
  assert.deepEqual(rendering.playing, { step: "Step 2 of 8", button: "Pause" });
  assert.deepEqual(rendering.played, { step: "Step 8 of 8", button: "Play" });
  // Shown batches read their screenshot once each; a menu pick has none to read.
  assert.ok(rendering.reads.includes("bench-00451") && rendering.reads.includes("bench-00001"));
  assert.ok(!rendering.reads.some((hash) => hash === null));
});
