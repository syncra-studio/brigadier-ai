import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

// The real sidebar in Chromium over a faked daemon, workspaces on: one row per workspace with
// the strongest state of its tabs and their count, and a click opening the tab had open last.
test("sidebar: a project's workspaces are rows with the strongest state of their tabs", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "workspaces.html", "workspaces-result", 20000)) as {
    rows: { title: string; status: string | null; active: boolean }[];
    opened: unknown;
    activeAfter: string[];
  };
  assert.deepEqual(
    seen.rows.map(({ title, status }) => [title, status]),
    [
      ["Redesigning the header", "working:2 working"],
      ["Fix the login", "ready:Ready"],
      ["Tidy the docs", "merged:Merged"],
      ["Waiting on a limit", "needsYou:Needs you"],
      ["Lone worker", "running:"],
    ],
  );
  assert.deepEqual(seen.opened, { type: "conversation", id: "w1-b" }, "the row opens the tab had open last");
  assert.deepEqual(seen.activeAfter, ["Redesigning the header"]);
});

test("closing a working session's tab stops nothing; it stays in the sidebar and its row opens it again", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "workspaces.html?close=1", "workspaces-result", 20000)) as Record<string, unknown>;
  assert.deepEqual(seen.opened, { type: "conversation", id: "w5-a" });
  assert.equal(seen.statusBefore, "running:");
  assert.deepEqual(seen.afterClose, { type: "draft", kind: "session", projectId: "p-1" }, "the project's new-session page shows in its place");
  assert.equal(seen.barAfter, false, "its tabs are closed");
  assert.deepEqual((seen.requestsOnClose as string[]).filter((method) => /interrupt|archive|stop|delete|close/i.test(method)), [], "closing asks the core for nothing that stops it");
  assert.equal(seen.statusAfter, "running:", "still working in the sidebar");
  assert.deepEqual(seen.reopened, { type: "conversation", id: "w5-a" }, "its row opens it again");
  assert.equal(seen.chatTab, "Lone worker");
});
