import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

test("a failed session turn reads calmly: one row, raw words under Details, Try again, no repeated notice", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(
    await renderFixturePage(t, "turn-error.html?view=done&approvals=0&summary=0&sidebar=0&drive=1", "turn-error-result", 10000),
  ) as {
    header: string | null;
    headerDestructive: boolean;
    rows: number;
    destructive: boolean;
    summary: string | null;
    detailsOpen: boolean | null;
    rawShownBefore: boolean;
    rawShownAfter: boolean;
    copy: string | null;
    notices: string[];
    tryAgain: boolean;
    sent: string[];
    bare: { text: string | null; details: number | null; buttons: number | null };
  };
  assert.match(seen.header ?? "", /^Stopped after /);
  assert.equal(seen.headerDestructive, false);
  assert.equal(seen.rows, 1);
  assert.equal(seen.destructive, false);
  assert.equal(seen.summary, "This turn stopped because of an error on Claude's side.");
  // The raw error shows nowhere until Details opens: not in the row, not above the composer.
  assert.equal(seen.detailsOpen, false);
  assert.equal(seen.rawShownBefore, false);
  assert.equal(seen.rawShownAfter, true);
  assert.equal(seen.copy, "Copy");
  assert.deepEqual(seen.notices, ["The sandbox isn't available here: commands run without it."]);
  // A session's Try again sends "Try again." the way the composer does.
  assert.equal(seen.tryAgain, true);
  assert.deepEqual(seen.sent, ["Try again."]);
  // A bare row: its line, no Details and nothing to press.
  assert.equal(seen.bare.text?.trim(), "The search failed.");
  assert.equal(seen.bare.details, 0);
  assert.equal(seen.bare.buttons, 0);
});

test("a turn Brigadier quit during reads interrupted, never as answered, and Continue resumes it", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(
    await renderFixturePage(t, "turn-error.html?view=done&approvals=0&summary=0&sidebar=0&drive=1&interrupted=1", "turn-error-result", 10000),
  ) as { header: string | null; summary: string | null; buttons: string[]; sent: string[]; resumed: number; destructive: boolean };
  assert.match(seen.header ?? "", /^Interrupted after /);
  assert.equal(seen.summary, "Interrupted: Brigadier quit while this was running. Continue picks it up where it stands.");
  assert.equal(seen.destructive, false);
  // Continue, not Try again; it resumes the request instead of sending a new message.
  assert.deepEqual(seen.buttons, ["Continue"]);
  assert.deepEqual(seen.sent, []);
  assert.equal(seen.resumed, 1);
});
