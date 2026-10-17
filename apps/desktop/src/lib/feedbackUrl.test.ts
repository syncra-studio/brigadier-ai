import assert from "node:assert/strict";
import { test } from "node:test";

import { detailsLine, feedbackTitle, feedbackUrl, MAX_URL } from "@/lib/feedbackUrl";

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
