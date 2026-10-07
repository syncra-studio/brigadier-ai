import assert from "node:assert/strict";
import { test } from "node:test";

import { editsLine, formatGrowth, growthRows } from "@/app/inspector/threadMetrics";
import type { RequestContext } from "@/ipc/generated";

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
