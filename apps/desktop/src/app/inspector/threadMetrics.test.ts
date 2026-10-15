import assert from "node:assert/strict";
import { test } from "node:test";

import { editsLine, formatGrowth, growthRows, summaryRows } from "@/app/inspector/threadMetrics";
import type { RequestContext, RequestSummary } from "@/ipc/generated";

test("the thread's own commits read as a count and the lines they changed", () => {
  const edits = { branch: "brigadier/s1/session", commits: 3, added: 1204, removed: 7, atMs: 1, kept: false };
  assert.equal(editsLine(edits), `3 commits on brigadier/s1/session: +${(1204).toLocaleString()} −7 lines`);
  assert.equal(editsLine({ ...edits, commits: 1, added: 2, removed: 0 }), "1 commit on brigadier/s1/session: +2 −0 lines");
  assert.match(editsLine({ ...edits, kept: true }), /\(branch merged; last count\)$/);
  assert.equal(editsLine({ ...edits, commits: 0, added: 0 }), "No commits of its own on brigadier/s1/session.");
  assert.equal(editsLine(null), "Its branch hasn't been looked at yet.");
});

function request(id: string, calls: number, first: number, last: number): RequestContext {
  return {
    requestId: id,
    preview: id === "r1" ? "" : `Request ${id}`,
    startedAtMs: 0,
    calls,
    firstTokens: first,
    lastTokens: last,
    growthTokens: last - first,
  };
}

test("context growth shows its sign, and nothing for a request of one call", () => {
  assert.equal(formatGrowth(0), "no change");
  assert.match(formatGrowth(5_700), /^\+5\.7K$/);
  assert.match(formatGrowth(-1_200), /^−1\.2K$/);
  const rows = growthRows([request("r1", 3, 12_400, 18_100), request("r2", 1, 20_000, 20_000), request("r3", 2, 21_000, 30_000)], 2);
  assert.deepEqual(
    rows.map((row) => [row.key, row.label, row.calls, row.growth]),
    [
      ["r3", "Request r3", "2 calls", "+9K"],
      ["r2", "Request r2", "1 call", ""],
    ],
  );
  assert.equal(rows[0]?.span, "21K → 30K");
  assert.equal(growthRows([request("r1", 3, 1, 2)], 5)[0]?.label, "An earlier request");
});

test("a request's summary reads its times and its tokens per provider and step, newest first", () => {
  const done: RequestSummary = {
    requestId: "r1",
    preview: "Fix the login",
    startedAtMs: 0,
    firstEventMs: 1_200,
    answerMs: 400_000,
    landedMs: 362_000,
    settledMs: 421_000,
    providers: [
      {
        provider: "claude",
        input: 10,
        cachedInput: 1_000_000,
        cacheWrite: 200_000,
        cacheWrite1h: 180_000,
        cacheWrite5m: 20_000,
        output: 100_000,
        raw: 1_300_010,
        rawWithoutCacheReads: 300_010,
        costUsd: 0.75,
      },
      { provider: "codex", input: 5, cachedInput: 0, cacheWrite: 0, cacheWrite1h: 0, cacheWrite5m: 0, output: 5, raw: 10, rawWithoutCacheReads: 10, costUsd: null },
    ],
    steps: [
      { step: "worker", raw: 1_000_000, calls: 3, turns: 0 },
      { step: "thread", raw: 300_020, calls: 9, turns: 2 },
      { step: "review", raw: 20, calls: 1, turns: 0 },
    ],
    cache: {
      missed: 300_000,
      bigMisses: 2,
      unestimatedCalls: 1,
      causes: [
        { cause: "note fork", sessions: 1, written: 180_000, missed: 252_000 },
        { cause: "fresh start", sessions: 2, written: 15_000, missed: 0 },
      ],
      unexplained: 48_000,
    },
  };
  const working: RequestSummary = {
    ...done,
    requestId: "r2",
    preview: "",
    firstEventMs: null,
    answerMs: null,
    landedMs: null,
    settledMs: null,
    providers: [],
    steps: [],
    cache: { missed: 0, bigMisses: 0, unestimatedCalls: 0, causes: [], unexplained: 0 },
  };
  const [newest, oldest] = summaryRows([done, working], 8);
  assert.equal(newest!.label, "An earlier request");
  assert.equal(newest!.times, "working");
  assert.deepEqual(newest!.providers, []);
  assert.equal(oldest!.times, "first 1.2 s · answer 6m 40s · landed 6m 2s · settled 7m 1s");
  assert.match(oldest!.providers[0]!, /^Claude 1\.3M raw, 300K without cache reads, \$0\.75$/);
  assert.match(oldest!.providers[1]!, /^Codex 10 raw, 10 without cache reads, 0% of input cached$/);
  // The thread's turns and the model calls per step (BUILD-PLAN A0).
  assert.match(oldest!.steps, /^worker 1M \(3 calls\) · thread 300K \(2 turns, 9 calls\) · review 20$/);
  // Where the cache writes came from, by lifetime and cause, the misses marked as estimates (BUILD-PLAN B0).
  assert.equal(
    oldest!.cache,
    "cache writes 200K (1h 180K, 5m 20K) · note fork wrote 180K, missed ≈252K · fresh start ×2 wrote 15K · ≈48K missed, no known cause · 2 calls missed over 20K",
  );
  assert.equal(newest!.cache, "");
  assert.equal(summaryRows([done, working], 1).length, 1);
  // A request cut off by a quit says so instead of "answer" (UI audit H1).
  const [interrupted] = summaryRows([{ ...done, ended: "interrupted", landedMs: null }], 1);
  assert.equal(interrupted!.times, "first 1.2 s · interrupted 6m 40s · settled 7m 1s");
});
