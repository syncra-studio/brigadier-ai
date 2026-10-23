import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

type Seen = { row: boolean; text: string; word: string; tone: string; colour: string; yellowToken: string; purpleToken: string; iconColour: string; red: boolean; buttons: number; afterStatus: number; afterPush: number; afterGhPr: number; authHint: string; polling: number; afterPollWord: string; afterPollReads: number; hiddenReads: number; terminalTimers: number; reopenedTimers: number; closedTimers: number; closedReads: number };
const card = async (t: test.TestContext, query: string) =>
  JSON.parse(await renderFixturePage(t, `pr-card.html?${query}`, "pr-card-result", 15000)) as Seen;

for (const [state, word, tone] of [
  ["failing", "Checks failed", "failed"],
  ["approved", "Ready", "good"],
  ["review", "Review needed", "yellow"],
  ["conflicts", "Review needed", "yellow"],
  ["pending", "Checks running", "yellow"],
  ["draft", "Draft", "quiet"],
  ["merged", "Merged", "purple"],
  ["closed", "Closed", "quiet"],
]) {
  test(`the PR row reads ${word} in its ${tone} colour, with no actions`, { timeout: 120000 }, async (t) => {
    const seen = await card(t, `state=${state}`);
    assert.equal(seen.row, true);
    assert.match(seen.text, /^#7 Add notes/);
    assert.deepEqual([seen.word, seen.tone, seen.buttons], [word, tone, 0]);
    assert.equal(seen.iconColour, seen.colour);
    assert.equal(seen.red, tone === "failed");
    if (tone === "yellow") assert.equal(seen.colour, seen.yellowToken);
    if (tone === "purple") assert.equal(seen.colour, seen.purpleToken);
    assert.equal(seen.polling, state === "merged" || state === "closed" ? 0 : 1);
  });
}

test("finished git push and gh pr items each refresh the row; git status does not", { timeout: 60000 }, async (t) => {
  const seen = await card(t, "state=approved&push=1");
  assert.deepEqual([seen.afterStatus, seen.afterPush, seen.afterGhPr], [0, 1, 2]);
});

test("a signed-out CLI shows the gh auth login hint", { timeout: 60000 }, async (t) => {
  const seen = await card(t, "state=signed-out");
  assert.equal(seen.row, false);
  assert.equal(seen.authHint, "gh auth login for PR status");
});

test("the 60 s poll changes Review needed to Ready with no push, survives a hidden card, and stops on merge or tab close", { timeout: 60000 }, async (t) => {
  const seen = await card(t, "state=review&poll=1");
  assert.equal(seen.word, "Review needed");
  assert.equal(seen.afterPollWord, "Ready");
  assert.deepEqual([seen.afterPollReads, seen.hiddenReads], [2, 3]);
  assert.equal(seen.terminalTimers, 0);
  assert.equal(seen.reopenedTimers, 1, "a gh pr command resumes polling after a terminal state");
  assert.equal(seen.closedTimers, 0);
  assert.equal(seen.closedReads, 5, "closing the tab makes no further calls");
});
