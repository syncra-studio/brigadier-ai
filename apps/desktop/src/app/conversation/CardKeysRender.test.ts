import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

type Seen = Record<string, unknown> & { calls: unknown[]; error?: string };
/** A 20px square, a little rounded, outlined. */
const square = (text: string) => ({ text, size: [20, 20], radius: 6, border: 1 });
const merge = (answer: string, input: string) => ({ method: "answerQuestion", cardId: "merge", answers: [answer], input });

test("every question card takes Enter only on the focused card and lights nothing until the user moves to an answer", { timeout: 90000 }, async (t) => {
  const results = JSON.parse(await renderFixtureLive(t, "card-keys.html", "card-keys-result", 60000)) as Record<string, Seen>;
  for (const [name, seen] of Object.entries(results)) assert.equal(seen.error, undefined, `${name}: ${seen.error}`);
  const { mountEnter, arrowEnter, click, enterOutside, plainEnterOutside, plainEnterFocused, digitUnfocused, digitFocused, closeAsk, closeMerge, escapeAsk } = results as Record<string, Seen>;

  // The dangerous one: the merge card focuses itself with its recommended answer, yet nothing
  // is lit, the pointer passing over lights nothing, and an Enter right after mount sends nothing.
  assert.equal(mountEnter!.focused, "card");
  assert.deepEqual(mountEnter!.lit, []);
  assert.deepEqual(mountEnter!.litHovered, []);
  assert.deepEqual(mountEnter!.calls, []);
  // Each answer's marker is a 20px outlined square with its number; the free-text row has a microphone.
  assert.deepEqual(mountEnter!.markers, [square("1"), square("2")]);
  assert.equal(mountEnter!.mic, "Dictate");

  assert.deepEqual(arrowEnter!.lit, [1]);
  assert.deepEqual(arrowEnter!.calls, [merge("Merge into main", "enter")]);

  assert.deepEqual(click!.calls, [merge("Not yet", "click")]);

  // A lit answer stays unsent by an Enter from the thread body, the side panel or the titlebar.
  assert.deepEqual(enterOutside!.lit, [1]);
  assert.deepEqual(enterOutside!.targets, ["thread", "side", "titlebar"]);
  assert.deepEqual(enterOutside!.calls, []);
  assert.deepEqual(enterOutside!.litAfter, [1]);

  // A plain question's answers may change code: its recommended answer is marked but not lit,
  // the pointer passing over lights nothing, and a click in the thread and Enter sends nothing.
  assert.deepEqual(plainEnterOutside!.lit, []);
  assert.equal(plainEnterOutside!.recommended, true);
  assert.deepEqual(plainEnterOutside!.litHovered, []);
  assert.equal(plainEnterOutside!.focused, "thread");
  assert.deepEqual(plainEnterOutside!.calls, []);
  assert.deepEqual(plainEnterOutside!.litAfter, []);
  // On the focused card, Enter right after mount sends nothing; 1 lights the first answer and
  // sends nothing, and Enter then sends it.
  assert.deepEqual(plainEnterFocused!.afterEnter, []);
  assert.deepEqual(plainEnterFocused!.lit, [1]);
  assert.deepEqual(plainEnterFocused!.afterDigit, []);
  assert.deepEqual(plainEnterFocused!.calls, [{ method: "answerQuestion", cardId: "ask", answers: ["Percent off"], input: "enter" }]);

  assert.equal(digitUnfocused!.focused, "thread");
  assert.deepEqual(digitUnfocused!.lit, []);
  assert.deepEqual(digitUnfocused!.calls, []);
  assert.deepEqual(digitFocused!.lit, [1], "1 on the focused card lights the first answer");
  assert.deepEqual(digitFocused!.calls, [], "and sends nothing");

  // × on a question reads Skip and answers it, so the request stops waiting (issue #239);
  // a merge card's × is its "Not yet". Esc still only puts the card aside.
  assert.equal(closeAsk!.title, "Skip");
  assert.deepEqual(closeAsk!.calls, [{ method: "answerQuestion", cardId: "close-ask", answers: ["Skipped: use your best judgment."], input: "click" }]);
  assert.deepEqual(closeMerge!.calls, [{ method: "answerQuestion", cardId: "close-merge", answers: ["Not yet"], input: "click" }]);
  assert.deepEqual(escapeAsk!.calls, [{ method: "dismiss" }]);
});

test("a question card keeps the answer the user lit and the text typed after a visit to Settings, takes a typed answer next to its options, and sends a multi-select question's marked answers together", { timeout: 90000 }, async (t) => {
  const { kept, typed, multi } = JSON.parse(await renderFixtureLive(t, "card-keys.html?case=kept", "card-keys-result", 60000)) as Record<string, Seen>;
  assert.equal(kept!.error, undefined, `kept: ${kept!.error}`);
  assert.notDeepEqual(kept!.litBefore, kept!.litFresh, "↓ lights an answer");
  assert.deepEqual(kept!.litAfter, kept!.litBefore);
  assert.equal(kept!.typedBefore, "tiers");
  assert.equal(kept!.typedAfter, "tiers");
  assert.deepEqual(kept!.litReopened, kept!.litFresh, "a question answered elsewhere keeps nothing");
  assert.equal(kept!.typedReopened, "");
  assert.deepEqual(kept!.calls, []);
  assert.equal(typed!.error, undefined, `typed: ${typed!.error}`);
  assert.deepEqual(typed!.calls, [{ method: "answerQuestion", cardId: "typed", answers: ["Both"], input: "typed" }]);
  // The CLI's own multi-select question (harness D7): checkboxes, marked by clicks, sent together.
  assert.equal(multi!.error, undefined, `multi: ${multi!.error}`);
  assert.deepEqual(multi!.roles, ["checkbox", "checkbox", "checkbox"]);
  assert.deepEqual(multi!.marked, [1, 2]);
  assert.deepEqual(multi!.beforeSend, []);
  assert.equal(multi!.button, "Submit");
  assert.deepEqual(multi!.calls, [{ method: "answerQuestion", cardId: "multi", answers: ["Percent off, Fixed amount"], input: "click" }]);
});
