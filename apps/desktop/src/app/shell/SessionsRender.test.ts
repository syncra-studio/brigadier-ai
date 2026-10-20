import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

// The real sidebar in Chromium over a faked daemon: a project lists its sessions, one row each
// with its own title and state, those that need the user first,
// and a click opens that session.
test("sidebar: a project's sessions are rows of their own, the sessions of an old workspace too, each with its state", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "sessions.html", "sessions-result", 20000)) as {
    rows: { title: string; status: string | null; tip: string | null; active: boolean; mark: string | null; words: string | null; error: boolean; plain: boolean }[];
    widths: { title: string; titleShare: number; stateWhole: boolean }[];
    projectHeader: { text: string; icons: number };
    opened: unknown;
    activeAfter: string[];
    rowsAfter: string[];
    projectHeaderAfter: { text: string; icons: number };
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
    assert.ok(row.words, `${row.title}: a plain state word`);
    assert.equal(row.mark, row.status?.startsWith("working:") || row.status?.startsWith("merging:") ? "spinner" : null, `${row.title}: its mark`);
    assert.equal(row.error, row.status?.startsWith("failed:"));
    assert.equal(row.plain, true, `${row.title}: no chip`);
  }
  // The title tells sessions apart: the state takes at most two fifths of the row.
  assert.equal(seen.widths.length, 12);
  for (const row of seen.widths) {
    assert.ok(row.titleShare >= 55, `${row.title}: the title keeps most of the row (${row.titleShare}%)`);
    assert.ok(row.stateWhole, `${row.title}: the state's words show whole`);
  }
  assert.deepEqual(seen.projectHeader, { text: "brigadier", icons: 1 }, "the project header contains only its folder icon and name");
  assert.deepEqual(seen.opened, { type: "conversation", id: "w1-a" }, "the row opens its own session");
  assert.deepEqual(seen.activeAfter, ["Header"]);
  assert.deepEqual(
    seen.rowsAfter,
    ["Deploy the site", "Broken build", "Old fix", "Footer", "Header", "Fix the login", "Tidy the docs", "Waiting on a limit", "Lone worker", "Ship the release", "Ready to review", "Paused task"],
    "a session that starts needing the user moves up",
  );
  assert.deepEqual(seen.projectHeaderAfter, seen.projectHeader, "sessions needing the user add no badge to the header");
  // A session's workers show in the session itself (context card, Workers panel, thread), never
  // under its row, though "Header" runs two.
  assert.deepEqual(seen.workers, [], "no worker rows in the sidebar");
});

test("the Projects flyout shows a folder and name without a count, preserving its session rows", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "sessions.html?flyout=1", "sessions-result", 20000)) as {
    projectHeader: { text: string; icons: number };
    sessions: string[];
  };
  assert.deepEqual(seen.projectHeader, { text: "brigadier", icons: 1 });
  assert.deepEqual(seen.sessions, ["Deploy the site", "Broken build", "Footer", "Header", "Fix the login", "Tidy the docs", "Waiting on a limit", "Lone worker", "Old fix", "Ship the release", "Ready to review", "Paused task"]);
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
  assert.ok((seen.chatTabStatusShare as number) <= 12, "only a mark takes space beside the title");
  assert.deepEqual(seen.notes, ["Still running. Reopen it from the sidebar."], "closing it leaves a small note");
  assert.equal(seen.dialog, false, "no dialog");
  assert.deepEqual(seen.idleNotes, [], "an idle session closes quietly");
});


for (const kind of ["session", "chat"]) for (const unread of [false, true]) {
  test(`${kind} rows preserve activity, error and unread marks with plain state words (${unread ? "unread" : "read"}); the tab shows the row's own marks without words`, { timeout: 60000 }, async (t) => {
    const seen = JSON.parse(await renderFixturePage(t, `sessions.html?states=1${kind === "chat" ? "&chats=1" : ""}${unread ? "&unread=1" : ""}`, "sessions-result", 20000)) as {
      title: string; rowWords: string; state: string; words: string; tip: string; rowTip: string; rowMark: string; mark: string | null; dot: boolean; tabError: boolean; width: number; titleFlex: string; beforeMark: string | null; error: boolean; plain: boolean; unreadSize: number | null; unreadSlot: number | null; spinnerSize: number | null; spinnerSlot: number | null; spinnerDuration: string | null; errorSize: number | null; errorSlot: number | null; leadingError: number | null;
    }[];
    assert.deepEqual([...new Set(seen.map((row) => row.rowWords))].toSorted(), ["Working", "Needs you", "Ready", "Merging", "Merged", "PR open", "Failed", "Stopped", "Reverted"].toSorted());
    for (const row of seen) {
      const busy = row.state === "working" || row.state === "merging";
      assert.equal(row.beforeMark, busy ? "spinner" : unread ? "unread" : null, `${row.title}: closed row mark`);
      assert.equal(row.rowMark, busy ? "spinner" : null, `${row.title}: the open row has no unread mark`);
      assert.equal(row.error, row.state === "failed", `${row.title}: errors have a separate leading icon`);
      assert.equal(row.plain, true, `${row.title}: no chip`);
      if (row.beforeMark === "unread") { assert.equal(row.unreadSize, 8); assert.equal(row.unreadSlot, 20); }
      if (busy) { assert.equal(row.spinnerSize, 16); assert.equal(row.spinnerSlot, 20); assert.equal(row.spinnerDuration, "2s"); }
      if (row.error) { assert.equal(row.errorSize, 16); assert.equal(row.errorSlot, 20); assert.equal(row.leadingError, 4); }
      // The tab shows exactly the open row's marks (no grey dot of its own), without the words.
      assert.equal(row.mark, row.rowMark, `${row.title}: the tab's mark is the row's`);
      assert.equal(row.dot, false, `${row.title}: no dot of the tab's own`);
      assert.equal(row.tabError, row.error, `${row.title}: the row's error glyph in the tab`);
      assert.equal(row.words, "", `${row.title}: no state word in the tab`);
      assert.equal(row.tip, `${row.title} · ${row.rowTip}`, `${row.title}: the whole tab has the state tooltip`);
      assert.ok(row.width <= 20, `${row.title}: only a mark beside the title`);
      assert.equal(row.titleFlex, "1", `${row.title}: the title takes the available width`);
    }
  });
}


test("a finished thread with no named state keeps its unread dot", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "sessions.html?unreadOnly=1", "sessions-result", 20000)) as { rows: { title: string; mark: string | null; words: string | null }[] };
  const row = seen.rows.find((each) => each.title === "Unread reply");
  assert.equal(row?.mark, "unread");
  assert.equal(row?.words, null);
});
