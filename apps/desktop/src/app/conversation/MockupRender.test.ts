import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixturePage } from "@/fixtures/headless";

test("a mockup gets its own session tab, busy while it builds, takes comments, sends them in one message and moves to the revision", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "mockup.html?drive=1", "mockup-result", 30000)) as Record<string, unknown>;
  if (process.env.MOCKUP_SEEN) console.log(JSON.stringify(seen, null, 2));
  assert.deepEqual(seen.building, { name: "Mockup, building", busy: true, active: "chat" });
  assert.deepEqual(seen.ready, { name: "Mockup", busy: false });
  assert.equal(seen.shown, true);
  assert.equal(seen.chip, false);
  assert.match(String(seen.pill), /Mockup · Screen 1.*v1 · Ready/);
  assert.match(String(seen.zoom), /^\d+%$/);
  assert.match(String(seen.helper), /Click on the image to add comments/);
  assert.equal(seen.placeholder, "Add a comment…");
  assert.deepEqual(seen.markers, ["1", "2"]);
  assert.match(String(seen.count), /2 comments.*Send/);
  assert.equal(seen.hiddenSend, false, "⌘↩ with the tab behind Chat sends nothing");
  assert.equal(seen.noteWhileSending, false, "no note is made while the notes go");
  assert.deepEqual(seen.attachments, ["mockup-1-notes.png"]);
  assert.equal(seen.sentTo, "mockup-fixture");
  assert.equal(seen.message,
    "Notes on the mockup (version 1), numbered as the markers on the attached screenshots:\n\n"
    + "Image 1 (mockup-1-notes.png):\n1. (x: 30%, y: 40%) Make this card blue\n2. (x: 85%, y: 85%) Taller footer band");
  assert.match(String(seen.working), /Working/);
  assert.equal(seen.notesLeft, 0);
  assert.match(String(seen.revised), /See latest changes/);
  assert.match(String(seen.kept), /v1 · Ready/);
  assert.match(String(seen.after), /v2 · Ready/);
  assert.match(String(seen.backTools), /Comment/);
  assert.deepEqual(seen.closed, { tab: false, active: "chat" });
  assert.deepEqual(seen.compared, { tab: true, name: "Mockup", busy: false });
});

test("a long session title leaves the Mockup tab its room in the strip", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "mockup.html?chat=1&long=1&measure=1", "mockup-result", 30000)) as { rem: number; chat: number; mockup: number };
  // --spacing-session-tab-max (15rem) and --spacing-session-tab-min (5.625rem).
  assert.ok(seen.chat <= 15 * seen.rem + 0.5, `the session's tab is at most 15rem: ${JSON.stringify(seen)}`);
  assert.ok(seen.mockup >= 5.625 * seen.rem - 0.5, `the Mockup tab keeps its least width: ${JSON.stringify(seen)}`);
});
