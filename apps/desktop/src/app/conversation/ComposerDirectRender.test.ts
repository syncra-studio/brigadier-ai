import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

test("the composer's To: sends one message straight to a live worker: picked from the @ menu's Message directly, past the session's paused queue, its draft back when refused, one at a time, then the worker's thread opens; queued messages stay the session's and leaving takes the To: away", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "composer-direct.html", "composer-direct-result", 90000)) as Record<string, unknown>;
  assert.equal(seen.error, undefined, String(seen.error));
  // Both workers can be mentioned; only the one still running can be messaged directly.
  assert.equal(seen.workers, 2);
  assert.deepEqual(seen.direct, ["Build the importer"]);
  // The pick is a To:, not a mention.
  assert.match(String(seen.chip), /To Build the importer/);
  assert.equal(seen.textAfterPick, "");
  assert.equal(seen.placeholder, "Message Build the importer");
  assert.equal(seen.pulled, false);
  // Refused: the message (its mention a chip again) and its To: come back.
  assert.deepEqual(seen.afterFailure, { text: "Use blue Build the importer", chip: true });
  // On its way, a second Enter sends nothing and keeps what was typed.
  assert.deepEqual(seen.whileSending, { sends: 1, text: "second" });
  assert.equal(seen.worker, "Worker: Build the importer");
  assert.equal(seen.pausedQuestion, false);
  // Each went to the worker with the ordinary mention as context; none to the session.
  const mention = JSON.stringify([{ type: "task", id: "task-live" }]);
  assert.deepEqual(seen.sent, [`worker task-live Use blue @task-1 ${mention}`, `worker task-live Use blue @task-1 ${mention}`]);
  // The worker's panel's Message sets the To: and puts the caret in the composer.
  assert.equal(seen.focusedAfterMessage, true);
  assert.equal(seen.chipAfterSwitch, false);
});
