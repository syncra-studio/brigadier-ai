import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

// The real sidebar in Chromium over a faked daemon: a project lists its sessions, one row each
// with its own title and state, and a click opens that session.
test("sidebar: a project's sessions are rows of their own, the sessions of an old workspace too, each with its state and why", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "sessions.html", "sessions-result", 20000)) as {
    rows: { title: string; status: string | null; active: boolean }[];
    opened: unknown;
    activeAfter: string[];
  };
  assert.deepEqual(
    seen.rows.map(({ title, status }) => [title, status]),
    [
      ["Footer", "working:Working · watching CI"],
      ["Header", "working:Working · 2 workers"],
      ["Fix the login", "ready:Ready · PR open, proof attached"],
      ["Tidy the docs", "merged:Merged"],
      ["Waiting on a limit", "working:Working · waiting for reset 14:05"],
      ["Deploy the site", "needsYou:Needs you · approval"],
      ["Lone worker", "working:Working"],
      ["Broken build", "needsYou:Needs you · failed"],
      ["Old fix", "reverted:Reverted"],
      ["Ship the release", "merging:Merging"],
    ],
  );
  assert.deepEqual(seen.opened, { type: "conversation", id: "w1-a" }, "the row opens its own session");
  assert.deepEqual(seen.activeAfter, ["Header"]);
});

test("closing a working session's tab stops nothing; it stays in the sidebar and its row opens it again", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "sessions.html?close=1", "sessions-result", 20000)) as Record<string, unknown>;
  assert.deepEqual(seen.opened, { type: "conversation", id: "w5-a" });
  assert.equal(seen.statusBefore, "working:Working");
  assert.deepEqual(seen.afterClose, { type: "draft", kind: "session", projectId: "p-1" }, "the project's new-session page shows in its place");
  assert.equal(seen.barAfter, false, "its tabs are closed");
  assert.deepEqual((seen.requestsOnClose as string[]).filter((method) => /interrupt|archive|stop|delete|close/i.test(method)), [], "closing asks the core for nothing that stops it");
  assert.equal(seen.statusAfter, "working:Working", "still working in the sidebar");
  assert.deepEqual(seen.reopened, { type: "conversation", id: "w5-a" }, "its row opens it again");
  assert.equal(seen.chatTab, "Lone worker");
  assert.equal(seen.chatTabStatus, "Working", "the chat tab says where it stands");
});
