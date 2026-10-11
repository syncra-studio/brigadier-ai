import assert from "node:assert/strict";
import { test } from "node:test";
import { createServer } from "vite";

import type { AccountView } from "@/ipc/generated";
import { renderFixturePage } from "@/fixtures/headless";

// The real page in Chromium over a faked daemon: emails masked, one row's revealed until the
// page is left, usage coloured only near the limit, the Don't give work rules (the old switch's
// and a new model's), and rows whose place moved brought into view, Advanced opening for them.
test("Providers masks emails, reveals one row until the page is left, and brings moved rows into view", { timeout: 60000 }, async (t) => {
  const dump = await renderFixturePage(t, "providers.html?drive=1", "providers-result", 12000);
  const seen = JSON.parse(dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&")) as Record<string, unknown>;
  assert.deepEqual(seen.sections, ["Claude Code", "Codex", "Who does what"]);
  assert.equal(seen.switchRow, true);
  const masked = ["ste•••••@•••••.com", "wor•••••@•••••.io", "Side project", "ste•••••@•••••.com"];
  assert.deepEqual(seen.masked, masked);
  assert.equal(seen.eyes, 4);
  assert.deepEqual(seen.usage, [
    { text: "5h32%", tone: "text-foreground/65" },
    { text: "Week69%", tone: "text-foreground/65" },
    { text: "5h84%", tone: "text-warning" },
    { text: "Week93%", tone: "text-destructive" },
  ]);
  assert.deepEqual(seen.models, [
    "Opus 5.5",
    "Sonnet 5.5Gets no tasks: a rule under Advanced keeps it to chats and sessions you pick it for.",
    "Fable 5.1Brigadier never gives work to Fable models. You can still pick one for a chat or session.",
    "GPT-6",
    "GPT-6 LunaNewBrigadier gives it no tasks until you allow it.Allow in Rules",
  ]);
  assert.equal(seen.advancedAtFirst, false);
  // Only the row whose eye was pressed, and only until the page is left.
  assert.deepEqual(seen.revealed, [masked[0], "work.person@corp.example.io", masked[2], masked[3]]);
  assert.deepEqual(seen.afterLeaving, masked);
  // Search's "Models Brigadier can give work to" lands on Rules, Advanced opened for it; the old
  // switch's rule and the new model's read as Don't give work rules.
  assert.deepEqual(seen.searched, {
    page: { type: "settings", page: "providers" },
    advanced: true,
    rulesInView: true,
    rules: [
      "Don't give Claude Sonnet 5.5 workYou can still pick it for a chat or session.",
      "Don't give Codex GPT-6 Luna workNew model: Brigadier gives it no tasks until you remove this rule.",
    ],
  });
  assert.equal(seen.advancedAgain, false);
  assert.deepEqual(seen.allowed, { advanced: true, rule: "Don't give Codex GPT-6 Luna work", inView: true });
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
