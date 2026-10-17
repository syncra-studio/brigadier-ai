import assert from "node:assert/strict";
import { test } from "node:test";
import { createServer } from "vite";

import type { AccountView } from "@/ipc/generated";
import { renderFixtureLive } from "@/fixtures/headless";

// The real page in Chromium over a faked daemon: a tab per agent, emails masked, one row's
// revealed until the page is left, usage coloured only near the limit, the models with their
// rule tags, and the links that moved to Routing (the old Usage page's, a rule's, a new model's).
test("Providers shows a tab per agent, masks emails, and links to Routing for rules", { timeout: 60000 }, async (t) => {
  const dump = await renderFixtureLive(t, "providers.html?drive=1", "providers-result", 30000);
  const seen = JSON.parse(dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&")) as Record<string, unknown>;
  assert.deepEqual(seen.tabs, ["ClaudeMax", "CodexPlus"]);
  assert.equal(seen.status, "Readyv2.1.0 · Max plan · /usr/local/bin/claudeOn");
  assert.equal(seen.switchRow, true);
  // The page's column is the one every Settings page has (#259).
  assert.equal(seen.settingsColumn, true);
  const masked = ["ste•••••@•••••.com", "wor•••••@•••••.io", "sid•••••@•••••.org"];
  assert.deepEqual(seen.masked, masked);
  assert.equal(seen.eyes, 3);
  assert.deepEqual(seen.usage, [
    { text: "32% used", tone: "text-foreground" },
    { text: "69% used", tone: "text-foreground" },
    { text: "84% used", tone: "text-warning" },
    { text: "93% used", tone: "text-destructive" },
  ]);
  assert.deepEqual(seen.models, [
    "Opus 5.5opus · defaultFrontier · Best for writing code, reviewing changes and resolving conflicts",
    "Sonnet 5.5sonnetNever gets Review changes · rule in RoutingGets no tasks: a rule in Routing keeps it to chats and sessions you pick it for",
    "Fable 5.1claude-fable-5-1Expensive: Brigadier uses it only where you pick it, in a chat, a session, your order or a rule",
  ]);
  // Only the row whose eye was pressed, and only until the page is left.
  assert.deepEqual(seen.revealed, [masked[0], "work.person@corp.example.io", masked[2]]);
  assert.deepEqual(seen.afterLeaving, masked);
  // The old Usage page's links land on the accounts.
  assert.deepEqual(seen.usageLink, { page: { type: "settings", page: "providers" }, accountsInView: true });
  // A model's rule tag opens Routing at its kind of work, the rule in view.
  assert.deepEqual(seen.ruleLink, {
    page: { type: "settings", page: "routing" },
    kind: "Review changes→ Opus 5.5, then GPT-6",
    inView: true,
  });
  assert.deepEqual(seen.codexModels, [
    "GPT-6gpt-6 · defaultFrontier · Best for writing code, reviewing changes and research",
    "GPT-6 Lunagpt-6-lunaNewBrigadier gives it no tasks until you allow itAllow in Routing",
    "GPT-5.5gpt-5.5Strong · Best for research, writing code and reviewing changes",
  ]);
  assert.deepEqual(seen.allowed, {
    page: { type: "settings", page: "routing" },
    rule: "Don't give Codex GPT-6 Luna work",
    inView: true,
  });
  // Routing's old name finds its page.
  assert.deepEqual(seen.searched, { type: "settings", page: "routing" });
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
