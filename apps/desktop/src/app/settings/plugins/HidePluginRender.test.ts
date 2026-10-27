import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixturePage } from "@/fixtures/headless";

test("native skills and servers confirm hiding for every agent, preserve failed removals and allow retry", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "../src/app/settings/plugins/Removal.fixture.html", "result", 15000)) as {
    error?: string; confirmation: string; canceledCalls: number; errorVisible: boolean; retainedAfterFailure: boolean;
    hiddenServer: boolean; skillConfirmation: string; removes: string[]; noRestore: boolean;
  };
  assert.equal(seen.error, undefined);
  for (const confirmation of [seen.confirmation, seen.skillConfirmation]) {
    assert.match(confirmation, /Brigadier and its sessions for all agents/);
    assert.match(confirmation, /files stay where Claude Code or Codex keep them/);
  }
  assert.equal(seen.canceledCalls, 0);
  assert.equal(seen.errorVisible, true);
  assert.equal(seen.retainedAfterFailure, true);
  assert.equal(seen.hiddenServer, true);
  assert.deepEqual(seen.removes, ["server", "server", "skill"]);
  assert.equal(seen.noRestore, true);
});
