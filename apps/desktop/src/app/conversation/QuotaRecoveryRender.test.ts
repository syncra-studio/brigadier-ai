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

for (const cause of ["automaticFallback", "manualSwitch", "quotaReset"]) test(`confirmed ${cause} dedupes replay and legacy notices while covering old reconstructed quota errors`, { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, `quota-recovery.html?transition=${cause}&failed=1&old=1&later=1&snapshot=1`, "quota-recovery-result", 60000));
  assert.equal(seen.error, undefined, JSON.stringify(seen));
  assert.equal(seen.strips, 0);
  assert.equal(seen.quotaErrors, 0);
  assert.equal(seen.replayedErrors, 0);
  assert.equal(seen.quotaSourceEnd, "failed");
  assert.equal(seen.dividers.length, 1);
  assert.match(seen.dividers[0], /Codex · GPT · Snapshot account/);
  assert.match(seen.dividers[0], cause === "automaticFallback" ? /Switched automatically to/ : cause === "manualSwitch" ? /Switched to/ : /Usage available · Resumed on/);
  if (cause === "manualSwitch") assert.doesNotMatch(seen.dividers[0], /automatically|couldn.t take/);
  assert.deepEqual(seen.transitionIds, ["confirmed-attempt-1"]);
  assert.deepEqual(seen.replayedDividers, seen.dividers);
  assert.equal(seen.keptDivider, true);
  assert.deepEqual(seen.seams, []);
  assert.equal(seen.resets, 0);
  assert.equal(seen.coveredModelNotices, 0);
  assert.equal(seen.unrelatedModelNotices, 1);
});

test("worker-scoped transitions retain worker attribution and never cover the lead's quota failure", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "quota-recovery.html?transition=automaticFallback&worker=1&failed=1&old=1&snapshot=1", "quota-recovery-result", 60000));
  assert.equal(seen.error, undefined, JSON.stringify(seen));
  assert.equal(seen.dividers.length, 1);
  assert.match(seen.dividers[0], /^Worker · Switched automatically to Codex/);
  assert.equal(seen.quotaErrors, 1);
  assert.equal(seen.coveredModelNotices, 1);
  assert.equal(seen.resets, 1);
  assert.deepEqual(seen.seams, ["Continued on Codex"]);
});

test("same-provider account fallback uses the launched account snapshot", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "quota-recovery.html?transition=automaticFallback&sameProvider=1&failed=1&snapshot=1", "quota-recovery-result", 60000));
  assert.equal(seen.error, undefined, JSON.stringify(seen));
  assert.equal(seen.dividers.length, 1);
  assert.match(seen.dividers[0], /Switched automatically to Claude Code · Opus · Snapshot account/);
  assert.equal(seen.quotaErrors, 0);
});

test("a queued/failed manual launch has no completed-transition divider; historical seams stay provider-only", { timeout: 90000 }, async (t) => {
  const pending = JSON.parse(await renderFixtureLive(t, "quota-recovery.html?transition=manualSwitch&pending=1&failed=1&snapshot=1&mode=automaticProbe&unknown=1", "quota-recovery-result", 60000));
  assert.equal(pending.error, undefined, JSON.stringify(pending));
  assert.deepEqual(pending.dividers, []);
  assert.match(pending.initial, /Checking again at.+Work resumes automatically when usage is available/);
  const old = JSON.parse(await renderFixtureLive(t, "quota-recovery.html?legacy=1&failed=1&snapshot=1", "quota-recovery-result", 60000));
  assert.deepEqual(old.seams, ["Continued on Codex"]);
  assert.deepEqual(old.dividers, []);
});

test("a confirmed transition does not hide a true network failure", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "quota-recovery.html?transition=automaticFallback&failed=1&unrelated=1&snapshot=1", "quota-recovery-result", 60000));
  assert.equal(seen.error, undefined, JSON.stringify(seen));
  assert.equal(seen.quotaErrors, 1);
  assert.equal(seen.replayedErrors, 1);
});

test("an unnamed own account keeps provider-only wording and the structured account in details", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "quota-recovery.html?transition=manualSwitch&own=1&failed=1&snapshot=1", "quota-recovery-result", 60000));
  assert.equal(seen.error, undefined, JSON.stringify(seen));
  assert.deepEqual(seen.dividers, ["Switched to Codex · GPT"]);
  assert.match(seen.accountDetails.join(" "), /Provider: Codex.*Account: Own account/);
  assert.doesNotMatch(seen.accountDetails.join(" "), /Snapshot account|this computer/);
});
