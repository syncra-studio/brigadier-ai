import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

for (const failed of [false, true]) test(`quota recovery ${failed ? "after native quota failure" : "with stale running lead"}: one strip, independent workers and direct manual switching`, { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, `quota-recovery.html${failed ? "?failed=1" : ""}`, "quota-recovery-result", 90000));
  assert.equal(seen.error, undefined, JSON.stringify(seen));
  assert.equal(seen.strips, 1);
  assert.equal(seen.oldCards, 0);
  assert.equal(seen.pills, 0);
  assert.equal(seen.quotaErrors, 0);
  assert.equal(seen.rawEnd, failed ? "failed" : null, "presentation never rewrites native failure to interruption");
  assert.match(seen.native, /subagent.*working/);
  assert.match(seen.initial, /Usage limit reached · Resets/);
  assert.doesNotMatch(seen.initial, /automatically|Resumes/, "old metadata supplies no automatic recovery promise");
  assert.match(seen.unknown, /Reset time unavailable/);
  assert.doesNotMatch(seen.unknown, /Resets \d|Resumes at/);
  assert.deepEqual(seen.initialLive, []);
  assert.match(seen.header, /Waiting for quota/);
  assert.match(seen.workerHeader, /Waiting for quota/);
  assert.equal(seen.staleToolShimmers, 0);
  assert.equal(seen.pausedTools, failed ? 0 : 1);
  assert.ok(seen.workerLive.some((line: string) => line.includes("Waiting for a worker")), JSON.stringify(seen.workerLive));
  assert.doesNotMatch(seen.workerLive.join(" "), /Thinking|Reconnecting/);
  for (const kind of ["credits", "spendControl"]) {
    assert.equal(seen[kind].cancels, false);
    assert.doesNotMatch(seen[kind].text, /Resumes|automatically|Resets/);
  }
  assert.match(seen.retryError, /still at their limits/);
  assert.deepEqual(seen.pickerCalls.map((call: { method: string }) => call.method), ["updateSetup", "continueOn"]);
  assert.equal(seen.pickerCalls[0].setup.orchestrator.model, "sonnet");
  assert.equal(seen.pickerCalls[1].provider, "codex");
  assert.equal(seen.pickerCalls[1].model.model, "gpt");
  assert.deepEqual(seen.feedback, ["Will continue on Codex after the current turn"]);
  assert.equal(seen.cancelled, true);
});

test("an unrelated failed turn keeps its diagnostics during a conversation quota wait", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "quota-recovery.html?failed=1&unrelated=1&snapshot=1", "quota-recovery-result", 60000));
  assert.equal(seen.error, undefined, JSON.stringify(seen));
  assert.equal(seen.quotaErrors, 1);
  assert.match(seen.header, /Stopped after/);
  assert.doesNotMatch(seen.header, /Interrupted/);
  assert.equal(seen.rawEnd, "failed");
});
