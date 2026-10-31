import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

type Read = {
  order: string[]; closed: string[];
  rows: { state: string; text: string; stop: boolean }[];
};

test("above the composer: one Workers strip, the CLI's own subagents in it as workers (D50), only while it has something live; its own commands show no strip (D24)", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "composer-rails.html?drive=1", "composer-rails-result", 60000)) as {
    emptyItems: number; all: Read; subagents: Read; checks: Read; panel: string; calls: string[];
  };
  assert.equal(seen.emptyItems, 0, "nothing live, no strip");
  assert.deepEqual(seen.all.order, ["Live work"]);
  assert.deepEqual(seen.all.closed, ["3 workers working · 1 waiting"]);

  // Subagents alone: the same Workers strip and rows, no strip of their own.
  assert.deepEqual(seen.subagents.order, ["Live work"]);
  assert.deepEqual(seen.subagents.closed, ["2 workers working"]);
  assert.deepEqual(seen.subagents.rows.map((row) => [row.state, row.stop]), [["working", true], ["working", true]]);
  assert.equal(seen.panel, "native:subagents:call-sa2", "a subagent's row opens its thread");

  // The session's own commands (tests, builds, waits): no strip.
  assert.deepEqual(seen.checks.order, []);
  // Stop all stops the workers and the CLI's own subagents.
  assert.deepEqual(seen.calls, ["stopBackgroundTask sa1", "stopBackgroundTask sa2", "stopWorkers"]);
});
