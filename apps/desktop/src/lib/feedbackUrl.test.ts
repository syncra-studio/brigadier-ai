import assert from "node:assert/strict";
import { test } from "node:test";

import type { FeedbackReport } from "@/ipc/generated";
import {
  detailsLine,
  feedbackTitle,
  feedbackUrl,
  MAX_URL,
  NEW_ISSUE_CHOOSER,
  repeatComment,
  reportBody,
  reportTitle,
} from "@/lib/feedbackUrl";

const details = { version: "0.1.0", system: "macOS 27.0 (arm64)", agents: ["Claude Code 2.1.4", "Codex 0.161.0"] };

test("a bug opens the bug form with what happened and the versions", () => {
  const url = new URL(feedbackUrl("bug", "  The tab keeps spinning\n\nafter I stop it.  ", details));
  assert.equal(url.origin + url.pathname, "https://github.com/syncra-studio/brigadier-ai/issues/new");
  assert.deepEqual(Object.fromEntries(url.searchParams), {
    template: "bug_report.yml",
    title: "[Bug]: The tab keeps spinning",
    actual: "The tab keeps spinning\n\nafter I stop it.",
    version: "0.1.0",
    environment: "macOS 27.0 (arm64), Claude Code 2.1.4, Codex 0.161.0",
  });
});

test("an idea opens the proposal form with the problem", () => {
  const url = new URL(feedbackUrl("idea", "Let me pin a chat", { version: null, system: null, agents: [] }));
  assert.deepEqual(Object.fromEntries(url.searchParams), {
    template: "proposal.yml",
    title: "[Proposal]: Let me pin a chat",
    problem: "Let me pin a chat",
  });
});

test("long text is cut so the link still opens, and the title to one short line", () => {
  for (const long of [`${"word ".repeat(30)}\n${"x".repeat(9000)}`, "漢字と絵文字🙂".repeat(800)]) {
    const link = feedbackUrl("bug", long, details);
    assert.ok(link.length <= MAX_URL && link.length > MAX_URL - 200, `${link.length} characters`);
    const actual = new URL(link).searchParams.get("actual")!;
    assert.ok(actual.endsWith("…") && long.startsWith(actual.slice(0, -1)), actual.slice(-20));
    // Cut by whole characters: no half of an emoji's pair.
    assert.ok(!/[\uD800-\uDBFF]…$/.test(actual));
  }
  assert.equal(feedbackUrl("idea", "short", details).includes("…"), false);
  assert.equal(feedbackTitle("  a   b \n c"), "a b");
  assert.equal(feedbackTitle("word ".repeat(30)).length, 72);
});

test("the details read as one line, without what isn't known", () => {
  assert.equal(detailsLine(details), "Brigadier 0.1.0, macOS 27.0 (arm64), Claude Code 2.1.4, Codex 0.161.0");
  assert.equal(detailsLine({ version: "0.1.0", system: null, agents: [] }), "Brigadier 0.1.0");
});

const bug: FeedbackReport = {
  kind: "bug",
  title: "Stop leaves the tab spinning",
  area: "Sessions, workers, landing or review",
  steps: "Not given; see What happened.",
  expected: "It stops.",
  actual: "The tab keeps spinning after Stop.",
  problem: "",
  today: "",
  wanted: "",
  duplicate: null,
  related: [],
};
const idea: FeedbackReport = {
  ...bug,
  kind: "idea",
  title: "Pin a chat",
  area: "",
  steps: "",
  expected: "",
  actual: "",
  problem: "Chats get lost in the list.",
  today: "",
  wanted: "A pin on a chat.",
};

test("a bug's issue body follows the bug form, says where it came from and ticks no box", () => {
  assert.equal(reportTitle(bug), "[Bug]: Stop leaves the tab spinning");
  assert.equal(
    reportBody(bug, details, true),
    [
      "_Filed from Brigadier’s Send feedback, written up from the reporter’s note. Brigadier searched open and closed issues for a duplicate first._",
      "### Version or commit\n\n0.1.0",
      "### Environment\n\nmacOS 27.0 (arm64), Claude Code 2.1.4, Codex 0.161.0",
      "### Area\n\nSessions, workers, landing or review",
      "### Steps to reproduce\n\nNot given; see What happened.",
      "### Expected\n\nIt stops.",
      "### What happened\n\nThe tab keeps spinning after Stop.",
      "### AI assistance\n\nWritten up by Brigadier from the reporter’s own words.",
    ].join("\n\n"),
  );
  assert.ok(!reportBody(bug, details, true).includes("[x]"));
  assert.ok(reportBody(bug, details, false).includes("couldn’t search the issues"));
});

test("a new issue names the issues it's close to but doesn't repeat, under where it came from", () => {
  const body = reportBody({ ...bug, related: [10457, 57] }, details, true);
  assert.ok(body.includes("duplicate first._\n\nMaybe related: #10457, #57\n\n### Version or commit"), body);
  assert.ok(!reportBody(bug, details, true).includes("Maybe related"));
});

test("an idea's issue body follows the proposal form; an empty field reads as GitHub writes it", () => {
  assert.equal(reportTitle(idea), "[Proposal]: Pin a chat");
  const body = reportBody(idea, details, true);
  assert.ok(body.endsWith(
    "### The problem\n\nChats get lost in the list.\n\n### What you do today\n\n_No response_\n\n### What should happen instead\n\nA pin on a chat.",
  ), body);
  assert.ok(!body.includes("Version"));
});

test("a repeat's comment carries its note and the versions it was seen on", () => {
  const repeat = { ...bug, duplicate: { number: 12, title: "Spinner", url: "u", open: true, comment: "Seen after Stop too." } };
  assert.equal(
    repeatComment(repeat, details),
    "_Reported again from Brigadier’s Send feedback._\n\nSeen after Stop too.\n\nSeen on: Brigadier 0.1.0, macOS 27.0 (arm64), Claude Code 2.1.4, Codex 0.161.0",
  );
});

test("the blank chooser is the new-issue page", () => {
  assert.equal(NEW_ISSUE_CHOOSER, "https://github.com/syncra-studio/brigadier-ai/issues/new/choose");
});
