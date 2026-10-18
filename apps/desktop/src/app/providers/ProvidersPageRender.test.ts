import assert from "node:assert/strict";
import { test } from "node:test";
import { createServer } from "vite";

import type { AccountView } from "@/ipc/generated";
import { renderFixtureLive } from "@/fixtures/headless";

// The real page in Chromium over a faked daemon: the agents with how each stands and its
// busiest limit, a row's switch that doesn't open it, an agent's page (Update when a newer CLI is
// out, Location's Change keeping only a real binary, every limit with when it resets, the models),
// emails masked with one revealed until the page or the agent's is left, the old Usage page's link landing on
// an agent's accounts, Routing found by its old name, and this computer's login added again.
test("Providers lists the agents, and an agent's page has its installation, accounts and models", { timeout: 60000 }, async (t) => {
  const dump = await renderFixtureLive(t, "providers.html?drive=1", "providers-result", 30000);
  const seen = JSON.parse(dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&")) as Record<string, unknown>;
  assert.equal(seen.title, "Providers");
  // The page's column is the one every Settings page has (#259).
  assert.equal(seen.settingsColumn, true);
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
  const masked = ["ste•••••@•••••.com", "wor•••••@•••••.io", "sid•••••@•••••.org"];
  assert.deepEqual(seen.accounts, [
    { title: masked[0], inUse: true, meters: ["5-hour32%Resets in 3h", "Weekly69%Resets in 1d 23h"] },
    { title: masked[1], inUse: false, meters: ["5-hour84%Resets in 3h", "Weekly93%Resets in 1d 23h"] },
    { title: masked[2], inUse: false, meters: ["5-hour32%Resets in 3h", "Weekly69%Resets in 1d 23h"] },
  ]);
  assert.equal(seen.eyes, 3);
  assert.deepEqual(seen.models, [
    "Opus 5.5opus  ·  defaultFrontier · Best for writing code, reviewing changes and resolving conflicts",
    "Sonnet 5.5sonnetStrong · Best for chat, writing code and research",
    "Fable 5.1claude-fable-5-1Expensive: Brigadier uses it only where you pick it, in a chat, a session, your order or a rule",
  ]);
  // Only the row whose eye was pressed, and only until the page is left.
  assert.deepEqual(seen.revealed, [masked[0], "work.person@corp.example.io", masked[2]]);
  // A file that isn't Claude Code is refused with why, and nothing is kept; a real one is kept.
  assert.deepEqual(seen.refused, {
    location: "Location/usr/local/bin/claude/opt/elsewhere/claude isn't Claude Code: it didn't say its versionChange",
    paths: [],
  });
  assert.deepEqual(seen.chosen, {
    location: "Location/opt/homebrew/bin/claudeChange",
    paths: [{ provider: "claude", path: "/opt/homebrew/bin/claude" }],
  });
  assert.equal(seen.cameBack, true);
  assert.deepEqual(seen.afterLeaving, masked);
  // The old Usage page's links land on an agent's accounts.
  assert.deepEqual(seen.usageLink, {
    page: { type: "settings", page: "providers" },
    agent: "ClaudeMax",
    accountsInView: true,
  });
  assert.equal(seen.back, true);
  // Going back to the agents hides a revealed email too.
  assert.deepEqual(seen.afterBack, masked);
  // Routing's old name finds its page.
  assert.deepEqual(seen.searched, { type: "settings", page: "routing" });
  // The same account as this computer's login and added here is one row: the added one, saying
  // it is this computer's login, in use when either is (#263).
  assert.deepEqual(seen.twin, [
    { title: "STE•••••@•••••.COM", own: true, inUse: true, menu: true },
    { title: masked[1], own: false, inUse: false, menu: true },
    { title: masked[2], own: false, inUse: false, menu: true },
  ]);
});

/** An extra Claude account named `name`, signed in as `email`. */
const view = (name: string, email: string): AccountView => ({
  account: { provider: "claude", account: "a" },
  name,
  default: false,
  status: { provider: "claude", path: "/c", version: null, loggedIn: true, authMethod: null, plan: null, email, guidance: null, compacts: false },
  quota: null,
  quotaAtMs: null,
  checkedAtMs: null,
  checking: false,
  checkedGeneration: 0,
  error: null,
});

// Renaming never shows an email the row hides: the editor starts empty for an account named by
// its email unless that row is revealed, and never starts from the signed-in email.
test("an account's name and editor keep its email masked unless its row is revealed", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { accountNames } = await server.ssrLoadModule("/src/app/providers/AccountRows.tsx");
    const named = view("work.person@corp.example.io", "me@example.com");
    assert.deepEqual(accountNames(named, false), { title: "wor•••••@•••••.io", email: "me•••••@•••••.com", editable: "" });
    assert.deepEqual(accountNames(named, true), {
      title: "work.person@corp.example.io",
      email: "me@example.com",
      editable: "work.person@corp.example.io",
    });
    // No name of its own: the title is the email, and the editor starts empty either way.
    const unnamed = view("", "me@example.com");
    assert.deepEqual(accountNames(unnamed, false), { title: "me•••••@•••••.com", email: null, editable: "" });
    assert.equal(accountNames(unnamed, true).editable, "");
    assert.equal(accountNames(view("me@example.com", "me@example.com"), true).editable, "");
    // A plain name shows and edits as it is.
    assert.deepEqual(accountNames(view("Side project", "s@example.org"), false), {
      title: "Side project",
      email: "s•••••@•••••.org",
      editable: "Side project",
    });
  } finally {
    await server.close();
  }
});
