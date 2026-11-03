import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

type Read = {
  order: string[]; closed: string[];
  rows: { state: string; text: string; stop: boolean }[];
};

test("above the composer: one Workers strip, only while it has something live; the CLI's own subagents are no workers (D96) and its own commands show no strip (D24)", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "composer-rails.html?drive=1", "composer-rails-result", 60000)) as {
    emptyItems: number; all: Read; subagents: Read; checks: Read; panel?: string; calls: string[];
  };
  assert.equal(seen.emptyItems, 0, "nothing live, no strip");
  assert.deepEqual(seen.all.order, ["Live work"]);
  assert.deepEqual(seen.all.closed, ["1 worker working · 1 waiting"]);

  // Subagents alone: no strip; each is a row of its parent's work (D96).
  assert.deepEqual(seen.subagents.order, []);
  assert.deepEqual(seen.subagents.rows, []);
  assert.equal(seen.panel, undefined);

  // The session's own commands (tests, builds, waits): no strip.
  assert.deepEqual(seen.checks.order, []);
  assert.deepEqual(seen.calls, []);
});
