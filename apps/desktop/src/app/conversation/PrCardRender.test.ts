import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

type Seen = { row: boolean; text: string; word: string; tone: string; red: boolean; buttons: number; afterStatus?: number; afterPush?: number };
const card = async (t: test.TestContext, query: string) =>
  JSON.parse(await renderFixturePage(t, `pr-card.html?${query}`, "pr-card-result", 15000)) as Seen;

// S14: the pull request row is `#N`, its title and its state in a word and a colour; it has no actions.
test("the pull request row shows its number, a state word and its colour, failed checks in red, and no buttons", { timeout: 120000 }, async (t) => {
  const failing = await card(t, "state=failing");
  assert.equal(failing.row, true);
  assert.match(failing.text, /^#7 Add notes/);
  assert.deepEqual([failing.word, failing.tone, failing.red, failing.buttons], ["Checks failed", "failed", true, 0]);
  const open = await card(t, "state=approved");
  assert.deepEqual([open.word, open.tone, open.red, open.buttons], ["Open", "good", false, 0]);
  const conflicts = await card(t, "state=conflicts");
  assert.deepEqual([conflicts.word, conflicts.tone, conflicts.buttons], ["Conflicts", "warning", 0]);
  const draft = await card(t, "state=draft");
  assert.deepEqual([draft.word, draft.tone, draft.buttons], ["Draft", "quiet", 0]);
  const merged = await card(t, "state=merged");
  assert.deepEqual([merged.word, merged.tone, merged.buttons], ["Merged", "quiet", 0]);
});

// The row reads `gh` again when a `git push` (or `gh pr`) command of the thread finishes, never on other commands.
test("after a git push item the pull request row reads gh again", { timeout: 60000 }, async (t) => {
  const seen = await card(t, "state=approved&push=1");
  assert.equal(seen.afterStatus, 0);
  assert.equal(seen.afterPush, 1);
});
