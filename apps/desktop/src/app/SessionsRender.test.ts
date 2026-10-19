import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

// The real sidebar in Chromium over a faked daemon: a project lists its sessions, one row each
// with its own title and state, those that need the user first with their count on the project,
// and a click opens that session.
test("sidebar: a project's sessions are rows of their own, the sessions of an old workspace too, each with its state", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "sessions.html", "sessions-result", 20000)) as {
    rows: { title: string; status: string | null; tip: string | null; active: boolean; mark: string | null; words: string | null }[];
    widths: { title: string; titleShare: number; stateWhole: boolean }[];
    count: { text: string; title: string } | null;
    opened: unknown;
    activeAfter: string[];
    rowsAfter: string[];
    countAfter: { text: string; title: string } | null;
    workers: string[];
  };
  // A row shows its state's keyword only; why it is in that state shows on hover.
  assert.deepEqual(
    seen.rows.map(({ title, status, tip }) => [title, status, tip]),
    [
      // Those that need the user float to the top, newest first like the rest.
      ["Deploy the site", "needsYou:Needs you", "Needs you · approval"],
      ["Broken build", "failed:Failed", "Failed"],
      ["Footer", "working:Working", "Working · watching CI"],
      ["Header", "working:Working", "Working"],
      ["Fix the login", "prOpen:PR open", "PR open · proof attached"],
      ["Tidy the docs", "merged:Merged", "Merged"],
      ["Waiting on a limit", "working:Working", "Working · waiting for reset 14:05"],
      ["Lone worker", "working:Working", "Working"],
      ["Old fix", "reverted:Reverted", "Reverted"],
      ["Ship the release", "merging:Merging", "Merging"],
      ["Ready to review", "ready:Ready", "Ready"],
      ["Paused task", "stopped:Stopped", "Stopped"],
    ],
  );
  for (const row of seen.rows) {
    assert.ok(row.words, `${row.title}: a state word or chip`);
    assert.equal(row.mark, row.status?.startsWith("working:") || row.status?.startsWith("merging:") ? "spinner" : "dot", `${row.title}: its mark`);
  }
  // The title tells sessions apart: the state takes at most two fifths of the row.
  assert.equal(seen.widths.length, 12);
  for (const row of seen.widths) {
    assert.ok(row.titleShare >= 55, `${row.title}: the title keeps most of the row (${row.titleShare}%)`);
    assert.ok(row.stateWhole, `${row.title}: the state's words show whole`);
  }
  assert.deepEqual(seen.count, { text: "2 need you", title: "2 need you" }, "the project counts the sessions that need the user");
  assert.deepEqual(seen.opened, { type: "conversation", id: "w1-a" }, "the row opens its own session");
  assert.deepEqual(seen.activeAfter, ["Header"]);
  assert.deepEqual(
    seen.rowsAfter,
    ["Deploy the site", "Broken build", "Old fix", "Footer", "Header", "Fix the login", "Tidy the docs", "Waiting on a limit", "Lone worker", "Ship the release", "Ready to review", "Paused task"],
    "a session that starts needing the user moves up",
  );
  assert.deepEqual(seen.countAfter, { text: "3 need you", title: "3 need you" });
  // A session's workers show in the session itself (context card, Workers panel, thread), never
  // under its row, though "Header" runs two.
  assert.deepEqual(seen.workers, [], "no worker rows in the sidebar");
});

test("closing a working session's tab stops nothing and says so in a note; it stays in the sidebar and its row opens it again", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "sessions.html?close=1", "sessions-result", 20000)) as Record<string, unknown>;
  assert.deepEqual(seen.opened, { type: "conversation", id: "w5-a" });
  assert.equal(seen.statusBefore, "working:Working");
  assert.deepEqual(seen.afterClose, { type: "draft", kind: "session", projectId: "p-1" }, "the project's new-session page shows in its place");
  assert.equal(seen.barAfter, false, "its tabs are closed");
  assert.deepEqual((seen.requestsOnClose as string[]).filter((method) => /interrupt|archive|stop|delete|close/i.test(method)), [], "closing asks the core for nothing that stops it");
  assert.equal(seen.statusAfter, "working:Working", "still working in the sidebar");
  assert.deepEqual(seen.reopened, { type: "conversation", id: "w5-a" }, "its row opens it again");
  assert.equal(seen.chatTab, "Lone worker");
  assert.equal(seen.chatTabStatus, "", "the tab shows no state word");
  assert.equal(seen.chatTabTip, "Lone worker · Working", "the tab's tooltip names the state");
  assert.equal(seen.chatTabMark, "spinner");
  assert.ok((seen.chatTabStatusShare as number) <= 10, "only a mark takes space beside the title");
  assert.deepEqual(seen.notes, ["Still running. Reopen it from the sidebar."], "closing it leaves a small note");
  assert.equal(seen.dialog, false, "no dialog");
  assert.deepEqual(seen.idleNotes, [], "an idle session closes quietly");
});


for (const kind of ["session", "chat"]) {
  test(`${kind} rows pair every state word with a mark; session tabs show only a mark and name the state in their tooltip`, { timeout: 60000 }, async (t) => {
    const seen = JSON.parse(await renderFixturePage(t, `sessions.html?states=1${kind === "chat" ? "&chats=1" : ""}`, "sessions-result", 20000)) as {
      title: string; rowWords: string; state: string; words: string; tip: string; rowTip: string; rowMark: string; mark: string; width: number; titleFlex: string;
    }[];
    assert.deepEqual([...new Set(seen.map((row) => row.rowWords))].toSorted(), ["Working", "Needs you", "Ready", "Merging", "Merged", "PR open", "Failed", "Stopped", "Reverted"].toSorted());
    for (const row of seen) {
      const mark = row.state === "working" || row.state === "merging" ? "spinner" : "dot";
      assert.equal(row.rowMark, mark, `${row.title}: row mark`);
      assert.equal(row.mark, mark, `${row.title}: tab mark`);
      assert.equal(row.words, "", `${row.title}: no state word in the tab`);
      assert.equal(row.tip, `${row.title} · ${row.rowTip}`, `${row.title}: the whole tab has the state tooltip`);
      assert.ok(row.width <= 16, `${row.title}: only a mark beside the title`);
      assert.equal(row.titleFlex, "1", `${row.title}: the title takes the available width`);
    }
  });
}
