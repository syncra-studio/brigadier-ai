import assert from "node:assert/strict";
import { test } from "node:test";

import { resumesAt } from "@/app/conversation/cards/resumesAt";
import type { LimitHit, QuotaWait } from "@/ipc/generated";
import { formatTime } from "@/lib/format";

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
