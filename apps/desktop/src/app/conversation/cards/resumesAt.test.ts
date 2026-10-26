import assert from "node:assert/strict";
import { test } from "node:test";

import { quotaRecoveryWords, resumesAt } from "@/app/conversation/cards/resumesAt";
import type { LimitHit, QuotaWait } from "@/ipc/generated";
import { formatDateTime, formatTime } from "@/lib/format";

const AT = 1_800_000_000_000;
const wait = (patch: Partial<QuotaWait> = {}): QuotaWait => ({ reason: "Claude's 5-hour window resets", resetsAtMs: AT, rule: null, ranking: null, sinceMs: 0, ...patch });
const limit = (kind: LimitHit["kind"]): LimitHit => ({ kind, resetsAtMs: AT, window: "five_hour" });

test("a usage window with a resume scheduled reads Resumes at, and resumes on its own", () => {
  assert.deepEqual(resumesAt(limit("usageWindow"), wait({ limitKind: "usageWindow" })), {
    title: `Resumes at ${formatTime(AT)}`, reason: "Claude's 5-hour window resets", atMs: AT, resumes: true,
  });
});

test("no wait (Don't resume, or none scheduled): no card", () => {
  assert.equal(resumesAt(limit("usageWindow"), null), null);
  assert.equal(resumesAt(null, null), null);
});

test("credits or a spend limit say so, with no time, and never resume on their own", () => {
  assert.deepEqual(resumesAt(limit("credits"), null), { title: "Out of credits", reason: null, atMs: null, resumes: false });
  assert.deepEqual(resumesAt(null, wait({ limitKind: "spendControl" })), { title: "Spend limit reached", reason: "Claude's 5-hour window resets", atMs: null, resumes: false });
});

test("a current unknown reset and kind never inherit the previous turn's limit", () => {
  const current = wait({ resetsAtMs: null });
  assert.deepEqual(resumesAt(limit("usageWindow"), current), {
    title: "Resumes when the limit resets", reason: current.reason, atMs: null, resumes: true,
  });
  assert.equal(resumesAt(limit("credits"), current)?.resumes, true);
});

test("automatic reset/probe copy uses the scheduled check, and never promises provider capacity", () => {
  for (const mode of ["automaticAt", "automaticProbe"] as const) {
    const words = quotaRecoveryWords(wait({ resetsAtMs: AT - 3_600_000, recovery: { mode, nextCheckAtMs: AT } }));
    assert.ok(words.detail?.includes(formatDateTime(AT)));
    assert.ok(!words.detail?.includes(formatDateTime(AT - 3_600_000)));
    assert.match(words.detail!, mode === "automaticAt" ? /Will try again automatically at/ : /Checking again at/);
    assert.match(words.detail!, /when usage is available/);
    assert.doesNotMatch(words.detail!, /Resumes at|will resume at/);
    assert.equal(words.cancelLabel, "Don't resume");
    assert.match(quotaRecoveryWords(wait({ resetsAtMs: null, recovery: { mode, nextCheckAtMs: null } })).detail!, /Checking usage automatically/);
  }
});

test("manual mode, credits/spend, and old missing metadata never promise automatic recovery", () => {
  for (const patch of [
    { recovery: { mode: "manualOnly" as const, nextCheckAtMs: AT } },
    { limitKind: "credits" as const, recovery: { mode: "automaticAt" as const, nextCheckAtMs: AT } },
    { limitKind: "spendControl" as const, recovery: { mode: "automaticProbe" as const, nextCheckAtMs: AT } },
  ]) {
    const words = quotaRecoveryWords(wait(patch));
    assert.doesNotMatch(`${words.title} ${words.detail}`, /automatic|checking again|resumes/i);
    assert.equal(words.cancelLabel, null);
  }
  const old = quotaRecoveryWords(wait());
  assert.match(old.title, /Resets/);
  assert.equal(old.cancelLabel, "Stop waiting");
  assert.equal(quotaRecoveryWords(wait({ resetsAtMs: null })).title, "Usage limit reached · Reset time unavailable");
});
