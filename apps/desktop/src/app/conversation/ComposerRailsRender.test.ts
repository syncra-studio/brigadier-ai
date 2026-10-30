import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

type Read = {
  order: string[]; closed: string[];
  subagents: { state: string; text: string; stop: boolean }[];
};

test("above the composer: Workers, then the session's Subagents, each only while it has something live; its own commands show no strip (D24)", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "composer-rails.html?drive=1", "composer-rails-result", 60000)) as {
    emptyItems: number; all: Read; subagents: Read; checks: Read; panel: string; calls: string[];
  };
  assert.equal(seen.emptyItems, 0, "nothing live, no strip");
  assert.deepEqual(seen.all.order, ["Live work", "Subagents"]);
  assert.deepEqual(seen.all.closed, ["1 worker working · 1 waiting", "2 subagents working"]);

  // Subagents: the CLI's own agents, out of the Workers strip, as plain rows.
  assert.deepEqual(seen.subagents.order, ["Subagents"], "no Workers strip for subagents alone");
  assert.deepEqual(seen.subagents.closed, ["2 subagents working"]);
  assert.deepEqual(seen.subagents.subagents.map((row) => [row.state, row.stop]), [["working", true], ["working", true]]);
  assert.equal(seen.panel, "native:subagents:call-sa2", "a subagent's row opens its thread");

  // The session's own commands (tests, builds, waits): no strip.
  assert.deepEqual(seen.checks.order, []);
  assert.deepEqual(seen.calls, ["stopBackgroundTask sa1", "stopBackgroundTask sa2"]);
});
