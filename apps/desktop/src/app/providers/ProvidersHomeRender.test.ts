import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

// The agents page drawn to the user's design (#262), not shown in the app yet: the overview's
// agents with how each stands and its busiest limit, a row's switch that doesn't open it, the
// agent's page (Update when a newer CLI is out, Location with Change that keeps only a real
// binary, every limit with when it resets), and Providers going back.
test("Providers' agents open their page, and a picked binary that isn't one is refused", { timeout: 60000 }, async (t) => {
  const dump = await renderFixtureLive(t, "providers.html?next=1&drive=1", "providers-next-result", 30000);
  const seen = JSON.parse(dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&")) as Record<string, unknown>;
  assert.equal(seen.title, "Providers");
  assert.equal(seen.fallback, true);
  assert.deepEqual(seen.agents, [
    { name: "Claude", state: "Ready · 3 accounts · v2.1.0", meter: "Weekly · in use69%" },
    { name: "Codex", state: "Ready · 1 account · v2.1.0", meter: "Weekly · in use69%" },
  ]);
  // Both are installed: nothing under Not set up, and Add another agent is there.
  assert.equal(seen.notSetUp, 0);
  assert.equal(seen.addAgent, true);
  assert.deepEqual(seen.afterSwitch, { page: false, disabled: ["codex"] });
  assert.equal(seen.header, "ClaudeMaxReady · Claude Code by AnthropicEnabledCheck again");
  assert.equal(seen.cli, "Claude Code CLIv2.1.0 · v2.1.4 is outUpdate");
  assert.equal(seen.updateEnabled, true);
  assert.equal(seen.location, "Location/usr/local/bin/claudeChange");
  assert.deepEqual(seen.accounts, [
    { title: "ste•••••@•••••.com", inUse: true, meters: ["5-hour32%Resets in 3h", "Weekly69%Resets in 1d 23h"] },
    { title: "wor•••••@•••••.io", inUse: false, meters: ["5-hour84%Resets in 3h", "Weekly93%Resets in 1d 23h"] },
    { title: "sid•••••@•••••.org", inUse: false, meters: ["5-hour32%Resets in 3h", "Weekly69%Resets in 1d 23h"] },
  ]);
  assert.deepEqual(seen.models, [
    "Opus 5.5opus  ·  defaultFrontier · Best for writing code, reviewing changes and resolving conflicts",
    "Sonnet 5.5sonnetStrong · Best for chat, writing code and research",
    "Fable 5.1claude-fable-5-1Expensive: Brigadier uses it only where you pick it, in a chat, a session, your order or a rule",
  ]);
  // A file that isn't Claude Code is refused with why, and nothing is kept; a real one is kept.
  assert.deepEqual(seen.refused, {
    location: "Location/usr/local/bin/claude/opt/elsewhere/claude isn't Claude Code: it didn't say its versionChange",
    paths: [],
  });
  assert.deepEqual(seen.chosen, {
    location: "Location/opt/homebrew/bin/claudeChange",
    paths: [{ provider: "claude", path: "/opt/homebrew/bin/claude" }],
  });
  assert.equal(seen.back, true);
});
