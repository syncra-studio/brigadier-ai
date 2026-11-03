import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

type Seen = { failed: string | null; headers: (string | null)[]; cards: (string | null)[]; buttons: (string | null)[]; notices: (string | null)[][]; bubbles: number };

test("a turn Brigadier quit under that went on by itself reads Interrupted with one quiet line, no card (D84)", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "quit-reopened.html", "quit-reopened-result", 10000)) as Seen;
  assert.equal(seen.failed, null);
  assert.match(seen.headers[0] ?? "", /^Interrupted after 15s$/);
  assert.match(seen.headers[1] ?? "", /^Worked for /);
  // No error card, nothing to press: the work carried on right below.
  assert.deepEqual(seen.cards, []);
  assert.deepEqual(seen.buttons, []);
  assert.deepEqual(seen.notices, [["Brigadier reopened · picked up where it stopped"], []]);
  // Brigadier's word to go on is no bubble of the user's.
  assert.equal(seen.bubbles, 1);
});

test("a turn Brigadier quit under that hasn't gone on keeps its card with Continue", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "quit-reopened.html?went=0", "quit-reopened-result", 10000)) as Seen;
  assert.equal(seen.failed, null);
  assert.match(seen.headers[0] ?? "", /^Interrupted after 15s$/);
  assert.deepEqual(seen.cards, ["Interrupted: Brigadier quit while this was running. Continue picks it up where it stands."]);
  assert.deepEqual(seen.buttons, ["Continue"]);
  assert.deepEqual(seen.notices, [[]]);
});
