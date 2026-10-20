import assert from "node:assert/strict";
import { test } from "node:test";

import { lastRefresh, refreshRunning } from "@/app/settings/routing/rankings";
import type { RankingsRefresh, RankingsRefreshState } from "@/ipc/generated";

const refresh = (state: RankingsRefreshState, patch: Partial<RankingsRefresh> = {}): RankingsRefresh => ({
  state,
  jobId: state === "idle" ? null : "job-1",
  startedAtMs: state === "idle" ? null : new Date(2026, 9, 4, 14, 5).getTime(),
  finishedAtMs: null,
  model: null,
  provider: null,
  baseRevision: null,
  changes: [],
  errors: [],
  overlayApplied: false,
  overlayAtMs: null,
  ...patch,
});

test("only a running refresh is polled", () => {
  assert.equal(refreshRunning("checkingRegistry"), true);
  assert.equal(refreshRunning("researching"), true);
  for (const state of ["idle", "done", "failed", "superseded", "cancelled"] as const) {
    assert.equal(refreshRunning(state), false, state);
  }
});

test("the last refresh: when, with which model and how it ended", () => {
  assert.equal(lastRefresh(refresh("idle"), null), null);
  assert.equal(lastRefresh(refresh("researching"), "Opus 4.5"), null);
  const finishedAtMs = new Date(2026, 9, 4, 14, 9).getTime();
  assert.match(lastRefresh(refresh("done", { finishedAtMs }), "Opus 4.5") ?? "", /^.+ with Opus 4\.5: finished$/);
  assert.match(lastRefresh(refresh("failed", { finishedAtMs }), null) ?? "", /: failed$/);
  assert.equal(lastRefresh(refresh("cancelled", { startedAtMs: null }), null), "stopped");
});
