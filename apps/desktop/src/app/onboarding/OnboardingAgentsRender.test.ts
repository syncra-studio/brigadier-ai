import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

const parse = (dump: string) =>
  JSON.parse(dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&")) as Record<string, unknown>;

// The real step in Chromium over a faked daemon: connected agents come first, each card offers
// the one thing it needs next. Install opens the agent's install guide; once it is installed
// (seen when the window comes back) the card offers Sign in and waits for the browser's sign-in;
// hovered, Waiting… reads Cancel in the same room and stops it. The card says a failure on its
// line, and turns Connected with its plan once the sign-in is noticed.
// The cards keep their places; the line under the title follows what's connected.
test("onboarding: an agent's card from Install to Connected", { timeout: 60000 }, async (t) => {
  const seen = parse(await renderFixtureLive(t, "onboarding-agents.html?drive=1", "onboarding-agents-result", 30000));
  assert.equal(seen.error, undefined);
  assert.deepEqual(seen.first, {
    cards: [
      { name: "Codex", line: "Pro plan", action: "Connected" },
      { name: "Claude Code", line: "Not installed", action: "Install" },
    ],
    subtitle: "Codex is ready. Connect any others you use, or start right away.",
    count: "1 of 2 connected",
    continue: true,
  });
  assert.deepEqual(seen.found, { name: "Claude Code", line: "Found on this computer · Not signed in", action: "Sign in" });
  assert.deepEqual(seen.waiting, { name: "Claude Code", line: "Complete sign-in in your browser", action: "Waiting…" });
  assert.deepEqual(seen.hover, {
    name: "Claude Code",
    line: "Complete sign-in in your browser",
    action: "Cancel",
    label: "Cancel sign-in to Claude Code",
    moved: false,
  });
  assert.deepEqual(seen.cancelled, { name: "Claude Code", line: "Found on this computer · Not signed in", action: "Sign in" });
  assert.deepEqual(seen.failed, { name: "Claude Code", line: "The sign-in didn't finish. Try again.", action: "Sign in" });
  assert.deepEqual(seen.connected, {
    cards: [
      { name: "Codex", line: "Pro plan", action: "Connected" },
      { name: "Claude Code", line: "Max plan", action: "Connected" },
    ],
    subtitle: "Claude Code and Codex are ready. Connect any others you use, or start right away.",
    count: "2 of 2 connected",
    continue: true,
  });
  assert.deepEqual(seen.asked, [
    "openUrl https://code.claude.com/docs/en/setup",
    "connectAgent claude",
    "cancelConnect claude",
    "connectAgent claude",
    "connectAgent claude",
  ]);
  assert.deepEqual(seen.jargon, []);
});

// With no agent connected, the one found on this computer comes first and Continue waits, on
// Enter too.
test("onboarding: no agent connected holds Continue", { timeout: 60000 }, async (t) => {
  const seen = parse(
    await renderFixtureLive(t, "onboarding-agents.html?drive=1&codex=signedOut", "onboarding-agents-result", 30000),
  );
  assert.deepEqual(seen.first, {
    cards: [
      { name: "Codex", line: "Found on this computer · Not signed in", action: "Sign in" },
      { name: "Claude Code", line: "Not installed", action: "Install" },
    ],
    subtitle: "Connect at least one agent to continue.",
    count: "0 of 2 connected",
    continue: false,
  });
  // Enter on Sign in is Continue's, held back: nothing signs in, nothing goes on.
  assert.deepEqual(seen.enter, { continued: 0, asked: [] });
});
