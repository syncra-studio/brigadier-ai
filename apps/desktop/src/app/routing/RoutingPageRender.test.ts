import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

// The real Routing page in Chromium over a faked daemon: the kinds of work with what each runs
// on, a kind's order as a sentence, its ranking right now (asked about the change's author), its
// rules and overrides, editing and resetting the order, and the models' ratings with a profile.
test("Routing shows each kind's order, ranking, rules and the models' ratings, and edits the order", { timeout: 60000 }, async (t) => {
  const dump = await renderFixtureLive(t, "routing.html?drive=1", "routing-result", 30000);
  const seen = JSON.parse(dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&")) as Record<string, unknown>;
  assert.deepEqual(seen.kinds, [
    "Explore the code→ Automatic · GPT-6 now",
    "Research→ Automatic · Opus 5.5 now",
    "Write code→ Opus 5.5, then GPT-6",
    "Review changes→ Opus 5.5, then GPT-6",
    "Resolve conflicts→ Automatic · Opus 5.5 now",
    "Run checks→ Automatic · Opus 5.5 now",
    "Use apps1 override→ GPT-6, then Opus 5.5",
    "Take over a chat→ the chat's model, then Automatic",
    "Coordinate work→ Automatic · Opus 5.5 now",
    "Write commit messages→ Automatic · Opus 5.5 now",
  ]);
  assert.equal(seen.selected, "Explore the code→ Automatic · GPT-6 now");
  assert.equal(seen.heading, "Review changes");
  assert.equal(
    seen.order,
    "Review changes tries⠿1Opus 5.5· highthen⠿2GPT-6· highAdd modelIf neither can run,pick automatically.⠿ Drag to reorder× Remove a model⌄ Change model or effort",
  );
  // Each part of the score in its column; the pick's reason under it; the four first of those
  // that can't take it, then how many more.
  assert.deepEqual(seen.ranked, [
    "1GPT-6Frontier9.00+0.400−0.259.15highPickedTop eligible score. Claude wrote this change, so Codex reviews it.",
    "2Opus 5.5Frontier9.10+0.60−3.8005.90high",
  ]);
  assert.equal(seen.blocked, 4);
  assert.equal(seen.more, "1 more that can't take it");
  assert.deepEqual(seen.rules, [
    "Don't give Claude Sonnet 5.5 work",
    "Don't give Codex GPT-6 Luna work",
    "Never use Claude Sonnet 5.5 for review",
  ]);
  assert.equal(seen.overrides, 0);
  assert.deepEqual(seen.unrevealable, []);
  // A review is ranked as if Claude wrote the change, then as Codex did once asked.
  assert.deepEqual(seen.authors, ["claude"]);
  assert.equal(seen.askedCodex, true);
  assert.equal(seen.reasonAfter, "Top eligible score.");
  // × removes a model; the order may wait; Reset goes back to Automatic and keeps the order.
  assert.deepEqual(seen.afterRemove, ["opus"]);
  assert.equal(seen.orderAfterRemove, "Review changes tries⠿1Opus 5.5· highAdd model");
  assert.equal(seen.waits, true);
  assert.deepEqual(seen.afterReset, { manual: false, kept: 1 });
  assert.equal(seen.automatic, "Review changes picks automatically — now GPT-6 · highAdd modelBring back your order: Opus 5.5");
  // Ratings: per provider, then those not in the registry; Fable left out; the best unfaded score
  // of each kind marked; a row opens its profile with what it learned.
  assert.deepEqual(seen.ratingGroups, ["Claude", "Codex", "Not in the registry"]);
  assert.deepEqual(seen.ratingRows, ["Opus 5.5", "Sonnet 5.5", "Opus 4.6", "GPT-6", "GPT-5.5", "GPT-6 Luna"]);
  assert.deepEqual(seen.best, ["9.2", "9.1", "9.0", "8.4", "9.0", "8.2", "7.6", "8.6", "6.8", "8.9", "6.8", "7.6", "6.8"]);
  assert.deepEqual(seen.profile, ["Profile", "Strength by kind", "Area adjustments", "In all projects"]);
  assert.equal(seen.learned, "+0.40");
  assert.equal(seen.form, true);
  assert.equal(seen.formAfterCancel, false);
});
