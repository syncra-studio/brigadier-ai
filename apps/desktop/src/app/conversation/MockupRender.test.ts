import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixturePage } from "@/fixtures/headless";

test("a mockup shows in its tab, takes comments, sends them in one message and moves to the revision", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "mockup.html?drive=1", "mockup-result", 30000)) as Record<string, unknown>;
  if (process.env.MOCKUP_SEEN) console.log(JSON.stringify(seen, null, 2));
  assert.match(String(seen.chip), /Mockup.*Ready/);
  assert.deepEqual(seen.tabs, ["Files", "Source", "Workers", "Running", "Mockup"]);
  assert.match(String(seen.pill), /Mockup · Screen 1.*v1 · Ready/);
  assert.match(String(seen.zoom), /^\d+%$/);
  assert.match(String(seen.helper), /Click on the image to add comments/);
  assert.equal(seen.placeholder, "Add a comment…");
  assert.deepEqual(seen.markers, ["1", "2"]);
  assert.match(String(seen.count), /2 comments.*Send/);
  assert.deepEqual(seen.attachments, ["mockup-1-notes.png"]);
  assert.equal(seen.sentTo, "mockup-fixture");
  assert.equal(seen.message,
    "Notes on the mockup (version 1), numbered as the markers on the attached screenshots:\n\n"
    + "Image 1 (mockup-1-notes.png):\n1. (x: 30%, y: 40%) Make this card blue\n2. (x: 85%, y: 85%) Taller footer band");
  assert.match(String(seen.working), /Working/);
  assert.equal(seen.notesLeft, 0);
  assert.match(String(seen.revised), /See latest changes/);
  assert.match(String(seen.after), /v2 · Ready/);
  assert.equal(seen.fullTab, "mockup");
  assert.equal(seen.fullShown, true);
  assert.deepEqual(seen.exited, { tabs: 0, sidebar: "expanded" });
});
