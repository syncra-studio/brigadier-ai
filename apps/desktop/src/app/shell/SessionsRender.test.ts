import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive, renderFixturePage } from "@/fixtures/headless";

type HoveredRow = {
  idle: { background: string; titleWidth: number; statusVisible: boolean; rowHeight: number; fontSize: number; actionWidth: number; iconWidth: number };
  hovered: { background: string; titleRight: number; titleWidth: number; actionsLeft: number; statusVisible: boolean; statusDisplay: string; overflowing: boolean };
  actions: { background: string; underPointer: boolean; tooltip: string; titleRight: number }[];
  menu: string[];
  copy: string[];
  navHoverBackground: string;
  after: { background: string; titleWidth: number };
};

for (const place of ["project sessions", "chats", "pinned sessions", "pinned chats"]) {
  const pinned = place.startsWith("pinned");
  const chat = place.endsWith("chats");
  const page = `sessions.html?hover=1${pinned ? "&pinned=1" : ""}${chat ? "&chats=1" : ""}`;

  test(`${place}: hovering with a mark gives the title all the room before the two actions`, { timeout: 60000 }, async (t) => {
    const seen = JSON.parse(await renderFixtureLive(t, page, "sessions-result", 20000)) as HoveredRow;
    assert.equal(seen.idle.rowHeight, 30);
    assert.equal(seen.idle.fontSize, 14);
    assert.equal(seen.idle.actionWidth, 20);
    assert.equal(seen.idle.iconWidth, 16);
    assert.equal(seen.idle.statusVisible, true);
    assert.equal(seen.hovered.statusVisible, false);
    assert.equal(seen.hovered.statusDisplay, "none", "the replaced state no longer takes title space");
    assert.equal(seen.hovered.overflowing, true, "a long title still truncates");
    const gap = seen.hovered.actionsLeft - seen.hovered.titleRight;
    assert.ok(gap >= 0 && gap <= 3, `only the existing clearance before the icons is reserved (${gap}px)`);
    assert.equal(seen.after.titleWidth, seen.idle.titleWidth, "the mark regains its usual space after leaving");
  });

  test(`${place}: the ⋯ and pin keep NavRow's exact hover background while their tooltips show`, { timeout: 60000 }, async (t) => {
    const seen = JSON.parse(await renderFixtureLive(t, page, "sessions-result", 20000)) as HoveredRow;
    assert.notEqual(seen.hovered.background, seen.idle.background, "the pointer gives an inactive row its hover fill");
    assert.equal(seen.hovered.background, seen.navHoverBackground, "the row uses NavRow's exact hover colour");
    for (const action of seen.actions) {
      assert.equal(action.underPointer, true, "the real pointer is on the action button");
      assert.equal(action.background, seen.navHoverBackground, `${action.tooltip}: the whole row keeps NavRow's hover colour`);
      assert.equal(action.titleRight, seen.hovered.titleRight, "moving onto actions does not shorten the title");
    }
    assert.deepEqual(seen.actions.map((action) => action.tooltip), [chat ? "Chat actions" : "Session actions", `${pinned ? "Unpin" : "Pin"} ${chat ? "chat" : "session"}`]);
    assert.equal(seen.after.background, seen.idle.background, "leaving the row clears the hover fill");
  });

  test(`${place}: the row's ⋯ opens its tab's menu`, { timeout: 60000 }, async (t) => {
    const seen = JSON.parse(await renderFixtureLive(t, page, "sessions-result", 20000)) as HoveredRow;
    assert.deepEqual(seen.menu, ["Rename⌥⌘R", "Copy", "Fork", "Put to sleep", "Archive⇧⌘A", "Permanently delete"]);
    assert.deepEqual(seen.copy, [chat ? "Copy chat id" : "Copy session id", "Copy as Markdown"]);
  });
}

test("a row's ⋯, its session not open, reads the thread to copy it as Markdown and fork its latest answer; with no answer there is nothing to fork; a session in its own worktree forks in it or in a new one", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "sessions.html?rowThread=1", "sessions-result", 30000)) as {
    error?: string; open: string; menu: string[]; markdown: string; forks: unknown[]; read: string[]; toasts: string[]; forksAfter: number;
    places: Record<string, string[]>;
  };
  assert.equal(seen.error, undefined);
  assert.equal(seen.open, "none", "no session is open");
  assert.deepEqual(seen.menu, ["Rename⌥⌘R", "Copy", "Fork", "Put to sleep", "Archive⇧⌘A", "Permanently delete"]);
  assert.equal(seen.markdown, "# Fix the login\n\n## User\n\nFix the login.\n\n## Assistant\n\nThe token expired; it refreshes now.\n\n## User\n\nAnd the logout?\n\n## Assistant\n\nLogout clears it too.\n");
  assert.deepEqual(seen.forks, [{ conversationId: "w2-a", messageId: "a2", place: "newWorktree" }]);
  // Read once, for Copy; Fork finds it held.
  assert.deepEqual(seen.read, ["getConversation w2-a", "threadView w2-a"]);
  assert.ok(seen.toasts.includes("Nothing to fork yet: the session has no answer."), JSON.stringify(seen.toasts));
  assert.equal(seen.forksAfter, 1);
  // D188: a session in its own worktree forks in that worktree too, on its branch.
  assert.deepEqual(seen.places, {
    "Fix the login": ["Fork here", "Fork to a new worktree"],
    "Tidy the docs": ["Fork here", "Fork to a new worktree"],
  });
});

// The real sidebar in Chromium over a faked daemon: a project lists its sessions, one row each
// with its own title and state, those that need the user first,
// and a click opens that session.
test("sidebar: a project's sessions are rows of their own, the sessions of an old workspace too, each with its state", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "sessions.html", "sessions-result", 20000)) as {
    rows: { title: string; status: string | null; tip: string | null; active: boolean; mark: string | null; words: string | null; error: boolean; plain: boolean }[];
    widths: { title: string; titleShare: number }[];
    projectHeader: { text: string; icons: number };
    opened: unknown;
    activeAfter: string[];
    rowsAfter: string[];
    projectHeaderAfter: { text: string; icons: number };
    workers: string[];
    cut: { overflows: boolean; ellipsis: string; mask: string; word: string | null };
    cutStopped: { overflows: boolean; ellipsis: string; mask: string; word: string | null };
  };
  // A row shows one mark and no state word (D3); what the mark means shows on hover.
  assert.deepEqual(
    seen.rows.map(({ title, status, tip }) => [title, status, tip]),
    [
      // Those that need the user float to the top, newest first like the rest.
      ["Deploy the site", "amber", "Needs you · approval"],
      ["Broken build", "amber", "Failed"],
      ["Ship the release", "amber", "Needs you · sign-in"],
      ["Footer", "working", "Working"],
      ["Header", "working", "Working"],
      ["Fix the login", null, null],
      ["Tidy the docs", null, null],
      ["Waiting on a limit", "working", "Waiting for usage limit · resets 14:05"],
      ["Lone worker", "working", "Working"],
      ["Old fix", null, null],
      ["Finished reply", null, null],
      // Stopped shows nothing (D11).
      ["Paused task", null, null],
      // A finished, read turn has no mark; git/PR state never gives it one.
      ["Draft the notes", null, null],
    ],
  );
  for (const row of seen.rows) {
    assert.equal(row.words, null, `${row.title}: no state word on a row`);
    assert.equal(row.mark, row.status === "working" ? "spinner" : row.status === "dot" ? "unread" : row.status === "amber" ? "amber" : null, `${row.title}: its mark`);
    assert.equal(row.error, false);
    assert.equal(row.plain, true, `${row.title}: no chip`);
  }
  // The title tells sessions apart: a mark takes only its 20-unit slot.
  assert.equal(seen.widths.length, 7, "the seven rows with a mark");
  for (const row of seen.widths) assert.ok(row.titleShare >= 80, `${row.title}: the title keeps the row (${row.titleShare}%)`);
  assert.deepEqual(seen.projectHeader, { text: "brigadier", icons: 1 }, "the project header contains only its folder icon and name");
  assert.deepEqual(seen.opened, { type: "conversation", id: "w1-a" }, "the row opens its own session");
  assert.deepEqual(seen.activeAfter, ["Header"]);
  assert.deepEqual(
    seen.rowsAfter,
    ["Deploy the site", "Broken build", "Old fix", "Ship the release", "Footer", "Header", "Fix the login", "Tidy the docs", "Waiting on a limit", "Lone worker", "Finished reply", "Paused task", "Draft the notes"],
    "a session that starts needing the user moves up",
  );
  assert.deepEqual(seen.projectHeaderAfter, seen.projectHeader, "sessions needing the user add no badge to the header");
  // A long title fades out at the end, a stopped row's as any other: no word beside it.
  for (const cut of [seen.cut, seen.cutStopped]) {
    assert.equal(cut.word, null);
    assert.equal(cut.overflows, true);
    assert.notEqual(cut.mask, "none");
  }
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
  assert.deepEqual(seen.sessions, ["Deploy the site", "Broken build", "Ship the release", "Footer", "Header", "Fix the login", "Tidy the docs", "Waiting on a limit", "Lone worker", "Old fix", "Finished reply", "Paused task", "Draft the notes"]);
});

test("closing a working session's tab stops nothing and says so in a note; it stays in the sidebar and its row opens it again", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "sessions.html?close=1", "sessions-result", 20000)) as Record<string, unknown>;
  assert.deepEqual(seen.opened, { type: "conversation", id: "w5-a" });
  assert.equal(seen.statusBefore, "working");
  assert.deepEqual(seen.afterClose, { type: "draft", kind: "session", projectId: "p-1" }, "the project's new-session page shows in its place");
  assert.equal(seen.barAfter, false, "its tabs are closed");
  assert.deepEqual((seen.requestsOnClose as string[]).filter((method) => /interrupt|archive|stop|delete|close/i.test(method)), [], "closing asks the core for nothing that stops it");
  assert.equal(seen.statusAfter, "working", "still working in the sidebar");
  assert.deepEqual(seen.reopened, { type: "conversation", id: "w5-a" }, "its row opens it again");
  assert.equal(seen.chatTab, "Lone worker");
  assert.equal(seen.chatTabStatus, "", "the tab shows no state word");
  assert.equal(seen.chatTabTip, "Lone worker · Working", "the tab's tooltip names the state");
  assert.equal(seen.chatTabMark, "spinner");
  assert.equal(seen.chatTabActions, "hidden", "the spinner holds the ⋯'s place until a hover");
  assert.ok((seen.chatTabStatusShare as number) <= 12, "only a mark takes space beside the title");
  assert.deepEqual(seen.notes, ["Still running. Reopen it from the sidebar."], "closing it leaves a small note");
  assert.equal(seen.dialog, false, "no dialog");
  assert.deepEqual(seen.idleNotes, [], "an idle session closes quietly");
});


for (const kind of ["session", "chat"]) for (const unread of [false, true]) {
  test(`${kind} rows show activity, error and unread marks and no state word (${unread ? "unread" : "read"}); the tab shows the row's own marks without words`, { timeout: 60000 }, async (t) => {
    const seen = JSON.parse(await renderFixturePage(t, `sessions.html?states=1${kind === "chat" ? "&chats=1" : ""}${unread ? "&unread=1" : ""}`, "sessions-result", 20000)) as {
      title: string; rowWords: string | null; state: string; openState: string | null; words: string; tip: string; rowTip: string | null; rowMark: string; mark: string | null; dot: boolean; tabError: boolean; width: number; titleFlex: string; beforeMark: string | null; error: boolean; plain: boolean; unreadSize: number | null; unreadSlot: number | null; spinnerSize: number | null; spinnerSlot: number | null; spinnerDuration: string | null; errorSize: number | null; errorSlot: number | null; leadingError: number | null;
    }[];
    assert.deepEqual([...new Set(seen.map((row) => row.rowWords))], [null], "no row has a state word (D3)");
    // A draft PR does not supply the tab's state word.
    assert.equal(seen.find((row) => row.title === "Draft the notes")?.tip, "Draft the notes");
    for (const row of seen) {
      const busy = row.state === "working";
      assert.equal(row.beforeMark, busy ? "spinner" : row.state === "dot" ? "unread" : row.state === "amber" ? "amber" : null, `${row.title}: closed row mark`);
      // In front of the user, a row shows no dot: its spinner if it works.
      assert.notEqual(row.openState, "dot", `${row.title}: no dot while open`);
      assert.equal(row.rowMark, row.openState === "working" ? "spinner" : null, `${row.title}: an open row has no dot`);
      assert.equal(row.error, false, `${row.title}: a failed row's one mark is amber, at its end`);
      assert.equal(row.plain, true, `${row.title}: no chip`);
      if (row.beforeMark === "unread") { assert.equal(row.unreadSize, 8); assert.equal(row.unreadSlot, 20); }
      if (busy) { assert.equal(row.spinnerSize, 16); assert.equal(row.spinnerSlot, 20); assert.equal(row.spinnerDuration, "2s"); }
      // The tab shows exactly the open row's marks (no grey dot of its own), without the words.
      assert.equal(row.mark, row.rowMark, `${row.title}: the tab's mark is the row's`);
      assert.equal(row.dot, false, `${row.title}: no dot of the tab's own`);
      assert.equal(row.tabError, row.rowTip === "Failed", `${row.title}: a failed tab keeps its error glyph`);
      assert.equal(row.words, "", `${row.title}: no state word in the tab`);
      assert.equal(row.tip, row.rowTip ? `${row.title} · ${row.rowTip}` : row.title, `${row.title}: the whole tab has the state tooltip`);
      assert.ok(row.width <= 20, `${row.title}: only a mark beside the title`);
      assert.equal(row.titleFlex, "1", `${row.title}: the title takes the available width`);
    }
  });
}


test("an idle session whose turn ended shows only its unread dot, no Done word", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "sessions.html?unreadOnly=1", "sessions-result", 20000)) as { rows: { title: string; mark: string | null; words: string | null }[] };
  const row = seen.rows.find((each) => each.title === "Unread reply");
  assert.equal(row?.mark, "unread");
  assert.equal(row?.words, null);
});


test("waiting questions show Needs you and the amber mark, also when an approval waits, but not while open; answers and skips clear it", { timeout: 60000 }, async (t) => {
  type Row = { status: string; tip: string; mark: string | null; active: boolean };
  const seen = JSON.parse(await renderFixturePage(t, "sessions.html?questions=1", "sessions-result", 20000)) as { before: Row[]; opened: Row; after: Row[] };
  for (const row of seen.before) {
    assert.equal(row.status, "amber");
    assert.ok(["Needs you · question", "Needs you · approval"].includes(row.tip));
    assert.equal(row.mark, "amber");
  }
  // The question is in front of the user: its row says what its turn does instead.
  assert.deepEqual({ status: seen.opened.status, tip: seen.opened.tip, mark: seen.opened.mark }, { status: "working", tip: "Working", mark: "spinner" });
  assert.equal(seen.opened.active, true);
  assert.deepEqual(seen.after.map(({ status, tip, mark }) => ({ status, tip, mark })), [
    { status: "working", tip: "Working", mark: "spinner" },
    { status: "amber", tip: "Needs you · approval", mark: "amber" },
  ]);
});


test("unopened sessions with working workers keep Working until idle, including reconnect snapshots; user requests still win", { timeout: 60000 }, async (t) => {
  type Row = { title: string; status: string; mark: string | null };
  const seen = JSON.parse(await renderFixturePage(t, "sessions.html?workerActivity=1", "sessions-result", 20000)) as { before: Row[]; someWorking: Row; idle: Row; reconnected: Row; threadReads: string[] };
  assert.deepEqual(seen.before.map(({ title, status, mark }) => [title, status, mark]), [
    ["Approval with workers", "amber", "amber"],
    ["Question with workers", "amber", "amber"],
    ["Workers still running", "working", "spinner"],
  ]);
  for (const row of [seen.someWorking, seen.reconnected]) {
    assert.equal(row.status, "working");
    assert.equal(row.mark, "spinner");
  }
  assert.equal(seen.idle.status, null, "no Done mark");
  assert.equal(seen.idle.mark, null);
  assert.deepEqual(seen.threadReads, [], "no sidebar session's thread was read");
});


test("a session that never ran has no mark", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "sessions.html?neverRan=1", "sessions-result", 20000)) as { rows: { title: string; status: string | null; mark: string | null }[] };
  const row = seen.rows.find((each) => each.title === "Never ran");
  assert.ok(row);
  assert.equal(row.status, null);
  assert.equal(row.mark, null);
});


test("Done uses the recorded turn-end unread dot: opening clears it through closing, and a later turn restores it", { timeout: 60000 }, async (t) => {
  type Row = { status: string; mark: string | null };
  const seen = JSON.parse(await renderFixturePage(t, "sessions.html?doneRead=1", "sessions-result", 20000)) as { before: Row; afterClose: Row; nextTurn: Row };
  assert.deepEqual([seen.before.status, seen.before.mark], ["dot", "unread"]);
  assert.deepEqual([seen.afterClose.status, seen.afterClose.mark], [null, null], "seen once opened, still seen after closing");
  assert.deepEqual([seen.nextTurn.status, seen.nextTurn.mark], ["dot", "unread"], "a later turn is a new reply");
});
