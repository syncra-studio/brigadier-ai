import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

// The real step in Chromium over a faked daemon: an agent that isn't on this computer has one
// Connect; it shows its install going, a plain failure with Try again, its browser sign-in with
// Cancel, and Connected once the sign-in is noticed. Continue waits for one connected agent
// only when none is; no developer words anywhere on the step.
test("onboarding: Connect installs a missing agent and signs it in", { timeout: 60000 }, async (t) => {
  const dump = await renderFixtureLive(t, "onboarding-agents.html?drive=1", "onboarding-agents-result", 30000);
  const seen = JSON.parse(dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&")) as Record<string, unknown>;
  assert.equal(seen.error, undefined);
  assert.deepEqual(seen.first, [
    { name: "Claude Code", button: "Connect", enabled: true, line: "Not connected", actions: [] },
    { name: "Codex", button: "Connected", enabled: true, line: "Connected · Pro plan", actions: [] },
  ]);
  // Codex is connected, so Continue is already on.
  assert.equal(seen.continueAtFirst, true);
  assert.deepEqual(seen.installing, {
    name: "Claude Code",
    button: "Connecting…",
    enabled: false,
    line: "Installing Claude Code… This takes a minute or two.",
    actions: ["Cancel"],
  });
  assert.deepEqual(seen.failed, {
    name: "Claude Code",
    button: "Try again",
    enabled: true,
    line: "Brigadier couldn't download Claude Code. Check your internet connection, then try again.",
    actions: [],
  });
  assert.deepEqual(seen.signingIn, {
    name: "Claude Code",
    button: "Connecting…",
    enabled: false,
    line: "Finish signing in to Claude Code in your browser.",
    actions: ["Cancel"],
  });
  assert.deepEqual(seen.cancelled, { name: "Claude Code", button: "Connect", enabled: true, line: "Not connected", actions: [] });
  assert.deepEqual(seen.connected, {
    name: "Claude Code",
    button: "Connected",
    enabled: true,
    line: "Connected · Max plan",
    actions: [],
  });
  assert.equal(seen.continueAtEnd, true);
  assert.deepEqual(seen.asked, [
    "connectAgent claude",
    "connectAgent claude",
    "cancelConnect claude",
    "connectAgent claude",
  ]);
  assert.deepEqual(seen.jargon, []);
});
