import assert from "node:assert/strict";
import { test } from "node:test";

import {
  changedModels,
  fieldWords,
  overlayStatus,
  refreshRunning,
  refreshStatus,
} from "@/app/routing/rankings";
import type { RankingsRefresh, RankingsRefreshState, RatingChange } from "@/ipc/generated";

const change = (model: string, fields: string[]): RatingChange => ({
  provider: "claude",
  model,
  fields,
  sources: ["https://example.com/card"],
});

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

test("a researched model whose rating didn't change isn't counted as changed", () => {
  const changes = [change("a", ["tier"]), change("b", []), change("c", ["strengths.review", "tier"])];
  assert.deepEqual(
    changedModels(changes).map((entry) => entry.model),
    ["a", "c"],
  );
  assert.equal(refreshStatus(refresh("done", { changes }))?.text, "Updated 2 models.");
  assert.equal(refreshStatus(refresh("done", { changes: [change("a", ["tier"])] }))?.text, "Updated 1 model.");
  assert.equal(refreshStatus(refresh("done", { changes: [change("b", [])] }))?.text, "No ratings changed.");
});

test("each state has its own words", () => {
  assert.equal(refreshStatus(refresh("idle")), null);
  assert.deepEqual(refreshStatus(refresh("checkingRegistry")), {
    tone: "running",
    text: "Checking for a newer model list…",
  });
  const researching = refreshStatus(refresh("researching", { model: "claude-opus-4-5" }));
  assert.equal(researching?.tone, "running");
  assert.match(researching?.text ?? "", /^Researching with claude-opus-4-5 since .+…$/);
  assert.equal(
    refreshStatus(refresh("researching", { startedAtMs: null }))?.text,
    "Researching with one of your models…",
  );
  assert.equal(refreshStatus(refresh("done"))?.tone, "done");
  assert.deepEqual(refreshStatus(refresh("failed")), {
    tone: "error",
    text: "The refresh failed. The ratings in use didn't change.",
  });
  assert.equal(refreshStatus(refresh("cancelled"))?.text, "The refresh was cancelled.");
  assert.match(refreshStatus(refresh("superseded"))?.text ?? "", /newer curated model list/);
});

test("the ratings in use: researched, no longer applied, or curated", () => {
  const at = new Date(2026, 9, 3, 9, 30).getTime();
  assert.match(overlayStatus(refresh("idle", { overlayApplied: true, overlayAtMs: at })), /^Using researched ratings from .+\.$/);
  assert.match(overlayStatus(refresh("superseded", { overlayAtMs: at })), /no longer applied/);
  assert.equal(overlayStatus(refresh("idle")), "Using the curated ratings.");
});

test("changed fields read as words", () => {
  assert.equal(fieldWords("tier"), "tier");
  assert.equal(fieldWords("strengths.review"), "review score");
  assert.equal(fieldWords("areaStrengths.frontend"), "frontend adjustment");
  assert.equal(fieldWords("defaultEffort.research"), "research effort");
});
