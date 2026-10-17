import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

type Seen = Record<string, unknown> & { calls: unknown[]; error?: string };
/** A 20px square, a little rounded, outlined. */
const square = (text: string) => ({ text, size: [20, 20], radius: 6, border: 1 });
const merge = (answer: string, input: string) => ({ method: "answerQuestion", cardId: "merge", answers: [answer], input });

test("every question card takes Enter only on the focused card, and consequential ones light nothing until the user moves to an answer; the phase plan's rail takes no Enter", { timeout: 90000 }, async (t) => {
  const results = JSON.parse(await renderFixtureLive(t, "card-keys.html", "card-keys-result", 60000)) as Record<string, Seen>;
  for (const [name, seen] of Object.entries(results)) assert.equal(seen.error, undefined, `${name}: ${seen.error}`);
  const { mountEnter, arrowEnter, click, enterOutside, plainEnterOutside, plainEnterFocused, digitUnfocused, digitFocused, phasePlan, implementPlan } = results as Record<string, Seen>;

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

  // A plain question lights its recommended answer, yet a click in the thread and Enter sends
  // nothing; Enter on the focused card sends it.
  assert.deepEqual(plainEnterOutside!.lit, [1]);
  assert.equal(plainEnterOutside!.focused, "thread");
  assert.deepEqual(plainEnterOutside!.calls, []);
  assert.deepEqual(plainEnterOutside!.litAfter, [1]);
  assert.deepEqual(plainEnterFocused!.calls, [{ method: "answerQuestion", cardId: "ask", answers: ["Percent off"], input: "enter" }]);

  assert.equal(digitUnfocused!.focused, "thread");
  assert.deepEqual(digitUnfocused!.lit, []);
  assert.deepEqual(digitUnfocused!.calls, []);
  assert.deepEqual(digitFocused!.lit, [1], "1 on the focused card lights the first answer");
  assert.deepEqual(digitFocused!.calls, [], "and sends nothing");

  assert.deepEqual(phasePlan!.targets, ["body", "rail"]);
  assert.deepEqual(phasePlan!.beforeEscape, [], "Enter never decides the phase plan");
  assert.deepEqual(phasePlan!.calls, [{ method: "dismiss" }], "while its Esc still puts it aside");

  assert.deepEqual(implementPlan!.lit, []);
  // "Yes, implement this plan" alone: an empty square.
  assert.deepEqual(implementPlan!.markers, [square("")]);
  assert.deepEqual(implementPlan!.afterEnter, [], "Enter right after mount builds nothing");
  assert.deepEqual(implementPlan!.calls, [{ method: "decidePlan", cardId: "document", approve: true, message: null }]);
});

test("a question card keeps the answer the user lit and the text typed after a visit to Settings", { timeout: 90000 }, async (t) => {
  const { kept } = JSON.parse(await renderFixtureLive(t, "card-keys.html?case=kept", "card-keys-result", 60000)) as Record<string, Seen>;
  assert.equal(kept!.error, undefined, `kept: ${kept!.error}`);
  assert.notDeepEqual(kept!.litBefore, kept!.litFresh, "↓ lights an answer");
  assert.deepEqual(kept!.litAfter, kept!.litBefore);
  assert.equal(kept!.typedBefore, "tiers");
  assert.equal(kept!.typedAfter, "tiers");
  assert.deepEqual(kept!.calls, []);
});
