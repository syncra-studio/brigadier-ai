import assert from "node:assert/strict";
import { test } from "node:test";

import {
  changedModels,
  fieldWords,
  lastRefresh,
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
  assert.equal(refreshStatus(refresh("done", { changes }), null)?.text, "Rankings refreshed. Updated 2 models.");
  assert.equal(
    refreshStatus(refresh("done", { changes: [change("a", ["tier"])] }), null)?.text,
    "Rankings refreshed. Updated 1 model.",
  );
  assert.equal(
    refreshStatus(refresh("done", { changes: [change("b", [])] }), null)?.text,
    "Rankings refreshed. No ratings changed.",
  );
});

test("each state has its own words", () => {
  assert.equal(refreshStatus(refresh("idle"), null), null);
  assert.deepEqual(refreshStatus(refresh("checkingRegistry"), null), {
    tone: "running",
    text: "Checking for a newer published model list…",
  });
  const researching = refreshStatus(refresh("researching"), "Opus 4.5");
  assert.equal(researching?.tone, "running");
  assert.match(researching?.text ?? "", /^Researching the models with Opus 4\.5 since .+…$/);
  assert.equal(
    refreshStatus(refresh("researching", { startedAtMs: null }), null)?.text,
    "Researching the models with one of your models…",
  );
  const finished = refreshStatus(refresh("done", { finishedAtMs: new Date(2026, 9, 4, 14, 9).getTime() }), null);
  assert.equal(finished?.tone, "done");
  assert.match(finished?.text ?? "", /^Rankings refreshed .+\. No ratings changed\.$/);
  const failed = refreshStatus(refresh("failed"), null);
  assert.equal(failed?.tone, "error");
  assert.match(failed?.text ?? "", /ratings didn't change/);
  assert.match(refreshStatus(refresh("cancelled"), null)?.text ?? "", /stopped/);
  assert.match(refreshStatus(refresh("superseded"), null)?.text ?? "", /newer published model list/);
});

test("the last refresh: when, with which model and how it ended", () => {
  assert.equal(lastRefresh(refresh("idle"), null), null);
  assert.equal(lastRefresh(refresh("researching"), "Opus 4.5"), null);
  const finishedAtMs = new Date(2026, 9, 4, 14, 9).getTime();
  assert.match(lastRefresh(refresh("done", { finishedAtMs }), "Opus 4.5") ?? "", /^.+ with Opus 4\.5: finished\.$/);
  assert.match(lastRefresh(refresh("failed", { finishedAtMs }), null) ?? "", /: failed\.$/);
  assert.equal(lastRefresh(refresh("cancelled", { startedAtMs: null }), null), "stopped.");
});

test("the ratings in use: researched, no longer applied, or published", () => {
  const at = new Date(2026, 9, 3, 9, 30).getTime();
  assert.match(overlayStatus(refresh("idle", { overlayApplied: true, overlayAtMs: at })), /^Researched ratings from .+\.$/);
  assert.match(overlayStatus(refresh("superseded", { overlayAtMs: at })), /isn't used/);
  assert.equal(overlayStatus(refresh("idle")), "The published ratings.");
});

test("changed fields read as words", () => {
  assert.equal(fieldWords("tier"), "tier");
  assert.equal(fieldWords("strengths.review"), "review score");
  assert.equal(fieldWords("areaStrengths.frontend"), "frontend adjustment");
  assert.equal(fieldWords("defaultEffort.research"), "research effort");
});
