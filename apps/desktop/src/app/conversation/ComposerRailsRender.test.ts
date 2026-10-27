import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

type Read = {
  order: string[]; closed: string[];
  subagents: { state: string; text: string; stop: boolean }[];
  checks: { state: string; text: string; waitedOn: boolean; stop: boolean; opens: boolean }[];
};

test("above the composer: Workers, then the session's Subagents, then its Checks nearest the composer, each only while it has something live", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "composer-rails.html?drive=1", "composer-rails-result", 60000)) as {
    emptyItems: number; all: Read; subagents: Read; checks: Read; opened: string; panel: string; calls: string[];
  };
  assert.equal(seen.emptyItems, 0, "nothing live, no strip");
  assert.deepEqual(seen.all.order, ["Live work", "Subagents", "Checks"]);
  assert.deepEqual(seen.all.closed, ["1 worker working · 1 waiting", "2 subagents working", "Waiting on Run full core tests · 1 check running"]);

  // Subagents: the CLI's own agents, out of the Workers strip, as plain rows.
  assert.deepEqual(seen.subagents.order, ["Subagents"], "no Workers strip for subagents alone");
  assert.deepEqual(seen.subagents.closed, ["2 subagents working"]);
  assert.deepEqual(seen.subagents.subagents.map((row) => [row.state, row.stop]), [["working", true], ["working", true]]);
  assert.equal(seen.panel, "native:subagents:call-sa2", "a subagent's row opens its thread");

  // Checks: the wait folded into the tests it watches, the smoke test queued behind the build.
  assert.deepEqual(seen.checks.closed, ["Waiting on Run full core tests · 2 checks running · 1 queued"]);
  const rows = seen.checks.checks;
  assert.deepEqual(rows.map((row) => row.state), ["running", "running", "queued", "running"]);
  assert.match(rows[0]!.text, /^Run full core tests·✓ parser\/reader.test.ts .*Session waits on this/);
  assert.deepEqual(rows.map((row) => row.waitedOn), [true, false, false, false]);
  assert.equal(rows.some((row) => row.text.startsWith("Wait for the full core tests")), false, "the folded wait has no row");
  assert.match(rows[2]!.text, /starts after Build the desktop app$/);
  assert.match(rows[3]!.text, /^Wait for the benchmark process to exit/, "a wait that names nothing is its own row");
  assert.deepEqual(rows.map((row) => row.stop), [true, true, true, true]);
  assert.equal(rows.every((row) => row.opens), true);
  assert.match(seen.opened, /^panel:output:task:ct3:\d+$/, "a live check opens what it printed with its generation");
  assert.deepEqual(seen.calls, ["stopBackgroundTask ct1", "stopBackgroundTask sa1", "stopBackgroundTask sa2"]);
});
