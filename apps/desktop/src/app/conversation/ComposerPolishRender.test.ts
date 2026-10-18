import assert from "node:assert/strict";
import { test } from "node:test";
import type { TestContext } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

async function polish(t: TestContext, which: string): Promise<Record<string, unknown>> {
  const seen = JSON.parse(await renderFixtureLive(t, `composer-polish.html?case=${which}`, "composer-polish-result", 60000)) as Record<string, unknown>;
  assert.equal(seen.error, undefined, String(seen.error));
  return seen;
}

test("Find in the chat keeps the keyboard: the composer focuses itself (on load, as a run starts) only when no other field has it (issue 304)", { timeout: 90000 }, async (t) => {
  const seen = await polish(t, "focus");
  assert.equal(seen.runStart, "Find in chat");
  assert.equal(seen.runStartFree, "Message input");
});

test("Send waits while attached files upload: disabled, its tip Files uploading…, and Enter sends nothing until they are stored (issue 302)", { timeout: 90000 }, async (t) => {
  const seen = await polish(t, "uploading");
  assert.deepEqual(seen.whileUploading, { disabled: true, tip: "Files uploading…" });
  assert.equal(seen.enterWhileUploading, false);
  assert.deepEqual(seen.afterUpload, { disabled: false, tip: "Send message" });
});

test("sending while the queue is paused asks Send message?: Send now sends it ahead of the paused queue, Keep paused queues it, Esc sends nothing (issue 303)", { timeout: 90000 }, async (t) => {
  const seen = await polish(t, "paused");
  assert.deepEqual(seen.asked, { title: "Send message?", buttons: ["Keep paused", "Send now", "Close"], sent: 0 });
  assert.deepEqual(seen.escaped, { sent: 0, kept: "first" });
  assert.deepEqual(seen.keptPaused, [{ text: "first", queueIndex: 1 }]);
  assert.deepEqual(seen.sentNow, [{ text: "second", queueIndex: null }]);
});

test("while a task runs the empty composer says Working for {time}, from a second in and ticking, and the usual placeholder once it ends (issue 297)", { timeout: 90000 }, async (t) => {
  const seen = await polish(t, "working");
  assert.equal(seen.idle, "Do anything");
  assert.match(String(seen.working), /^Working for 2m [5-9]s$/);
  assert.match(String(seen.ticked), /^Working for 2m \d+s$/);
  assert.notEqual(seen.ticked, seen.working);
  assert.equal(seen.done, "Do anything");
  assert.equal(seen.justStarted, "Do anything");
  assert.equal(seen.secondIn, "Working for 1s");
});

test("Esc puts the cursor in the composer (keeps it there when it has it) and cancels dictation that is getting going (issue 298)", { timeout: 90000 }, async (t) => {
  const seen = await polish(t, "esc");
  assert.equal(seen.inComposer, "Message input");
  assert.equal(seen.fromNowhere, "Message input");
  assert.equal(seen.dictating, "downloading");
  assert.deepEqual(seen.afterEsc, { dictate: "dictate", cancelled: true });
});

test("in Plan mode, Backspace at the very start of the composer turns Plan mode off and keeps the text; elsewhere it deletes (issue 299)", { timeout: 90000 }, async (t) => {
  const seen = await polish(t, "plan");
  assert.deepEqual(seen.midText, { text: "ab", plan: true, sets: [] });
  assert.deepEqual(seen.atStart, { text: "ab", plan: false, sets: [false] });
});

test("a file dragged over the thread shows a soft accent tint and a small Drop to attach pill, no grey veil or dashed frame (issue 300)", { timeout: 90000 }, async (t) => {
  const seen = await polish(t, "drop");
  const overlay = seen.overlay as Record<string, unknown>;
  assert.equal(overlay.dashed, false);
  assert.equal(overlay.blurred, false);
  assert.equal(overlay.tinted, true);
  assert.equal(overlay.pill, "Drop to attach");
  assert.equal(overlay.pillSmall, true);
});

test("Shift while dropping files on the thread types their paths at the caret, no overlay, nothing attached; the window's own drops: plain attaches, off the thread adds folders (issue 301)", { timeout: 90000 }, async (t) => {
  const seen = await polish(t, "shift");
  assert.equal(seen.pageOverlay, false);
  assert.equal(seen.pageText, "see /Users/me/notes v2.txt");
  assert.deepEqual(seen.nativeShift, { overlay: false, veil: false });
  assert.equal(seen.nativeText, "see /Users/me/notes v2.txt /Users/me/a.txt");
  assert.deepEqual(seen.nativePlain, { overlay: true, veil: false });
  assert.deepEqual(seen.attached, ["a.png image/png"]);
  assert.equal(seen.afterDrop, false);
  assert.deepEqual(seen.offThread, { overlay: false, veil: true });
  assert.deepEqual(seen.folders, ["/Users/me/repo"]);
});

test("a message sent while a dropped file is still being read waits for it: the file goes with that message and the next draft is empty (issue 330)", { timeout: 90000 }, async (t) => {
  const seen = await polish(t, "dropSend");
  assert.deepEqual(seen.whileReading, { sent: 0, disabled: true, tip: "Files uploading…" });
  assert.deepEqual(seen.messages, [{ text: "see this", attachments: ["a.png"] }]);
  assert.deepEqual(seen.nextDraft, { text: "", attachments: 0 });
  assert.deepEqual(seen.removedWhileUploading, { disabled: false, tip: "Send message" });
});
