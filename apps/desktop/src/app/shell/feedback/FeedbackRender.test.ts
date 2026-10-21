import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

// The sidebar foot's Help menu opens Send feedback; the user only writes and sends. What Brigadier
// wrote up is filed as the user (a repeat as a comment on its issue) and listed with its links;
// signed out each form opens in the browser; with no writer, the note opens in one form as before.
test("send feedback files what the user wrote and lists it with its links", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "feedback.html?drive=1", "feedback-result", 20000)) as Record<
    string,
    unknown
  >;
  assert.equal(seen.opened, true);
  assert.equal(seen.title, "Send feedback");
  assert.equal(seen.description, "Say what’s wrong or what you’d like. Brigadier writes it up for its developers.");
  assert.equal(seen.kinds, 0, "no kind to pick");
  assert.equal(seen.placeholder, "What’s wrong, or what would you like? Several things at once is fine.");
  assert.equal(seen.focused, true);
  assert.equal(seen.account, "Posts as @stephen-golban on GitHub, with your app and macOS version.");
  // It opens at its size, and its words don't change once shown (#470).
  assert.ok((seen.firstHeight as number) > 0);
  assert.equal(seen.firstHeight, seen.settledHeight);
  assert.deepEqual(seen.accountLines, [seen.account]);
  // No Cancel: the X closes it.
  assert.deepEqual(seen.footer, ["Write it on GitHub yourself", "Send"]);
  assert.equal(seen.sendDisabled, true);
  // The design's card: 600 wide.
  assert.equal(seen.width, 600);
  assert.equal(seen.sendEnabled, true);
  assert.deepEqual(seen.working, [
    "Writing up your reports…Splitting your note and checking GitHub for ones already reported",
    ["Sending…"],
  ]);
  // Closed while it works, it carries on; opened again, it shows what was filed.
  assert.deepEqual(seen.closedWhileWorking, [false, "working"]);
  assert.deepEqual(seen.asked, ["writeUpFeedback", "fileFeedback"]);
  assert.equal(seen.wrote, "The tab keeps spinning after Stop, and the usage bar lags. Also, let me pin a chat.");
  assert.deepEqual(seen.filed, [
    ["[Bug]: Stop leaves the tab spinning", null],
    ["", 12],
    ["[Proposal]: Pin a chat", null],
  ]);
  assert.deepEqual(seen.body, [
    "_Filed from Brigadier’s Send feedback, written up from the reporter’s note. Brigadier searched open and closed issues for a duplicate first._",
    "### Version or commit",
    "0.1.0",
  ]);
  assert.equal(seen.sentTitle, "Thanks, it’s sent");
  assert.equal(seen.sentDescription, "3 reports, filed on GitHub.");
  assert.deepEqual(seen.rows, [
    "Stop leaves the tab spinning#601",
    "Usage bar lags after a session endsAdded to #12",
    "Pin a chat#603",
  ]);
  assert.deepEqual(seen.sentFooter, ["Done"]);
  assert.deepEqual(seen.links, [
    "https://github.com/syncra-studio/brigadier-ai/issues/new/choose",
    "https://github.com/syncra-studio/brigadier-ai/issues/601",
  ]);
  assert.equal(seen.stillOpen, false);
  assert.equal(seen.signedOutAccount, "You’re not signed in to GitHub, so each report opens in your browser, filled in.");
  assert.deepEqual(seen.signedOut, [
    "Ready to post",
    "3 reports opened in your browser. Post each one there.",
    [
      "Stop leaves the tab spinningOpened in your browser",
      "Usage bar lags after a session endsAlready reported: #12",
      "Pin a chatOpened in your browser",
    ],
    ["bug_report.yml", "proposal.yml"],
  ]);
  assert.deepEqual(seen.failed, [
    "The tab keeps spinning after Stop.",
    "Couldn’t send it: HTTP 502: Bad gateway. Your note is still here.",
    ["Write it on GitHub yourself", "Try again"],
  ]);
  // Try again files what was written again; it isn't written up anew.
  assert.deepEqual(seen.retried, [
    ["fileFeedback"],
    ["Stop leaves the tab spinning#601", "Usage bar lags after a session endsAdded to #12", "Pin a chat#603"],
  ]);
  assert.deepEqual(seen.fallback, [
    {
      template: "bug_report.yml",
      title: "[Bug]: The tab keeps spinning",
      actual: "The tab keeps spinning\nafter I stop the session.",
      version: "0.1.0",
      environment: "macOS 27.0 Tahoe (aarch64), Claude Code 2.1.4",
    },
  ]);
  assert.deepEqual(seen.fallbackRows, ["Opened in your browser. Post it there.", ["The tab keeps spinningOpened in your browser"]]);
  assert.equal(seen.draft, "Let me pin a chat");
});

// Drawn with the app's shared parts (#385): the app's dialog with its title, line and close
// button, its field, divider and buttons.
test("send feedback is drawn with the app's dialog, field and buttons", { timeout: 60000 }, async (t) => {
  const pairs = JSON.parse(await renderFixturePage(t, "feedback.html?look=1", "feedback-look", 10000)) as Record<
    string,
    [unknown, unknown]
  >;
  for (const [part, [mine, shared]] of Object.entries(pairs)) {
    assert.notEqual(shared, null, `${part}: the shared part is drawn`);
    assert.deepEqual(mine, shared, `${part} looks like the app's own`);
  }
});
