import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

const parse = (dump: string) =>
  JSON.parse(dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&")) as Record<string, unknown>;

// The real step in Chromium over a faked daemon: connected agents come first, each card offers
// the one thing it needs next. Install is Connect, as in Settings › Providers: the agent's own
// installer runs, the card says so and waits, then waits for the browser's sign-in; hovered,
// Waiting… reads Cancel in the same room and stops it, the agent left installed and Sign in
// offered. The card says a failure on its line, and turns Connected with its plan once the
// sign-in is noticed. No install guide opens.
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
  assert.deepEqual(seen.installing, { name: "Claude Code", line: "Installing Claude Code…", action: "Waiting…" });
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
  assert.deepEqual(seen.asked, ["connectAgent claude", "cancelConnect claude", "connectAgent claude", "connectAgent claude"]);
  assert.deepEqual(seen.jargon, []);
});

// Install is Connect on every platform: the card installs, waits for the sign-in and turns
// Connected, and no guide opens.
for (const platform of ["macos", "linux", "windows"]) {
  test(`onboarding: on ${platform} Install installs and signs in`, { timeout: 60000 }, async (t) => {
    const seen = parse(
      await renderFixtureLive(
        t,
        `onboarding-agents.html?drive=1&install=1&platform=${platform}`,
        "onboarding-agents-result",
        30000,
      ),
    );
    assert.equal(seen.error, undefined);
    assert.deepEqual(seen.installing, { name: "Claude Code", line: "Installing Claude Code…", action: "Waiting…" });
    assert.deepEqual(seen.waiting, { name: "Claude Code", line: "Complete sign-in in your browser", action: "Waiting…" });
    assert.deepEqual(seen.connected, { name: "Claude Code", line: "Max plan", action: "Connected" });
    assert.deepEqual(seen.asked, ["connectAgent claude"]);
  });
}

// On a Linux with neither curl nor wget, Connect can't download the installer: the card says
// what is missing in full (not cut to one line), offers Install again, and links the vendor's
// install guide, which opens.
test("onboarding: with neither curl nor wget the card names them and links the guide", { timeout: 60000 }, async (t) => {
  const seen = parse(
    await renderFixtureLive(
      t,
      "onboarding-agents.html?drive=1&nodownloader=1&platform=linux",
      "onboarding-agents-result",
      30000,
    ),
  );
  assert.equal(seen.error, undefined);
  assert.deepEqual(seen.noDownloader, {
    line: "Installing Claude Code needs curl or wget, and neither is installed. Install one (for example `sudo apt install curl`) and try again, or install Claude Code yourself.",
    wraps: true,
    guide: "Claude Code's install guide",
    buttons: ["Claude Code's install guide", "Install"],
  });
  assert.deepEqual(seen.asked, ["connectAgent claude", "openUrl https://code.claude.com/docs/en/setup"]);
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

// When the browser can't hand Claude Code's sign-in back, its card offers the page with the code
// and a field for it; Enter in the field sends the code to the sign-in, not Continue (#321).
test("onboarding: a code pasted from the sign-in's page goes to the sign-in", { timeout: 60000 }, async (t) => {
  const seen = parse(
    await renderFixtureLive(t, "onboarding-agents.html?drive=1&claude=signedOut&code=1", "onboarding-agents-result", 30000),
  );
  assert.equal(seen.error, undefined);
  assert.deepEqual(seen.offered, {
    name: "Claude Code",
    line: "Complete sign-in in your browser",
    action: "Waiting…",
    says: "Browser didn't finish? Open the code page, then paste the code it shows.",
    placeholder: "Paste the code from the page",
    send: false,
  });
  assert.deepEqual(seen.typed, { send: true });
  assert.deepEqual(seen.sent, {
    asked: ["openUrl https://claude.com/cai/oauth/authorize?code=true", "sendConnectCode claude abc#xyz"],
    continued: 0,
  });
  assert.deepEqual(seen.connected, { name: "Claude Code", line: "Max plan", action: "Connected", field: false });
});
