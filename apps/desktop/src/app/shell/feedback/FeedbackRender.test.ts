import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

// The sidebar foot's Help menu opens Send feedback; a bug written there opens GitHub's bug form, filled in,
// and the dialog closes.
test("send feedback opens a filled-in GitHub issue", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "feedback.html?drive=1", "feedback-result", 10000)) as Record<
    string,
    unknown
  >;
  assert.equal(seen.opened, true);
  assert.equal(seen.title, "Send feedback");
  assert.equal(seen.description, "Opens a GitHub issue with your note filled in.");
  assert.equal(seen.footer, "You’ll review it before it’s posted");
  assert.deepEqual(seen.kinds, [
    ["Something’s wrong", "on"],
    ["An idea", "off"],
  ]);
  assert.equal(seen.placeholder, "What happened, and what did you expect?");
  assert.equal(seen.focused, true);
  // The details go with it, as the user sees them; a CLI without a known version is left out.
  assert.deepEqual(seen.included, ["Brigadier 0.1.0", "macOS 27.0 Tahoe (aarch64)", "Claude Code 2.1.4"]);
  assert.equal(seen.sendDisabled, true);
  // The design's card: 600 wide.
  assert.equal(seen.width, 600);
  assert.equal(seen.ideaPlaceholder, "What are you trying to do, and what gets in the way?");
  assert.equal(seen.sendEnabled, true);
  const urls = seen.opened_urls as string[];
  assert.equal(urls.length, 1);
  // The result is read from the page's markup, so its "&" comes escaped.
  const url = new URL(urls[0]!.replaceAll("&amp;", "&"));
  assert.equal(url.origin + url.pathname, "https://github.com/syncra-studio/brigadier-ai/issues/new");
  assert.deepEqual(Object.fromEntries(url.searchParams), {
    template: "bug_report.yml",
    title: "[Bug]: The tab keeps spinning",
    actual: "The tab keeps spinning\nafter I stop the session.",
    version: "0.1.0",
    environment: "macOS 27.0 Tahoe (aarch64), Claude Code 2.1.4",
  });
  // It closes: the rest happens on GitHub.
  assert.equal(seen.stillOpen, false);
  assert.deepEqual(seen.draft, { kind: "An idea", text: "Let me pin a chat" });
});

// Drawn with the app's shared parts (#385): the app's dialog with its title, line and close
// button, its choice pills, field, badges, divider and buttons.
test("send feedback is drawn with the app's dialog, pills, field, badges and buttons", { timeout: 60000 }, async (t) => {
  const pairs = JSON.parse(await renderFixturePage(t, "feedback.html?look=1", "feedback-look", 10000)) as Record<
    string,
    [unknown, unknown]
  >;
  for (const [part, [mine, shared]] of Object.entries(pairs)) {
    assert.notEqual(shared, null, `${part}: the shared part is drawn`);
    assert.deepEqual(mine, shared, `${part} looks like the app's own`);
  }
});
