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

  test(`${place}: hovering with a state word gives the title all the room before the two actions`, { timeout: 60000 }, async (t) => {
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
    assert.ok(seen.hovered.titleWidth > seen.idle.titleWidth, "the hidden Working word and spinner give their space back to the title");
    assert.equal(seen.after.titleWidth, seen.idle.titleWidth, "the state word regains its usual space after leaving");
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
    assert.deepEqual(seen.menu, ["Rename⌥⌘R", "Copy", "Fork", "Archive⇧⌘A", "Permanently delete"]);
    assert.deepEqual(seen.copy, [chat ? "Copy chat id" : "Copy session id", "Copy as Markdown"]);
  });
}

test("a row's ⋯, its session not open, reads the thread to copy it as Markdown and fork its latest answer; with no answer there is nothing to fork; a session in its own worktree forks only into a new one", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "sessions.html?rowThread=1", "sessions-result", 30000)) as {
    error?: string; open: string; menu: string[]; markdown: string; forks: unknown[]; read: string[]; toasts: string[]; forksAfter: number;
    places: Record<string, string[]>;
  };
  assert.equal(seen.error, undefined);
  assert.equal(seen.open, "none", "no session is open");
  assert.deepEqual(seen.menu, ["Rename⌥⌘R", "Copy", "Fork", "Archive⇧⌘A", "Permanently delete"]);
  assert.equal(seen.markdown, "# Fix the login\n\n## User\n\nFix the login.\n\n## Assistant\n\nThe token expired; it refreshes now.\n\n## User\n\nAnd the logout?\n\n## Assistant\n\nLogout clears it too.\n");
  assert.deepEqual(seen.forks, [{ conversationId: "w2-a", messageId: "a2", place: "newWorktree" }]);
  // Read once, for Copy; Fork finds it held.
  assert.deepEqual(seen.read, ["getConversation w2-a", "threadView w2-a"]);
  assert.ok(seen.toasts.includes("Nothing to fork yet: the session has no answer."), JSON.stringify(seen.toasts));
  assert.equal(seen.forksAfter, 1);
  // #648: "the same folder" of a session in its own worktree is that worktree, which it keeps.
  assert.deepEqual(seen.places, {
    "Fix the login": ["Fork in a new worktree"],
    "Tidy the docs": ["Fork in the same folder", "Fork in a new worktree"],
  });
});

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
    cut: { overflows: boolean; ellipsis: string; mask: string; clear: boolean | null; word: string | null };
    cutStopped: { overflows: boolean; ellipsis: string; mask: string; clear: boolean | null; word: string | null };
  };
  // A row shows its state's keyword only; why it is in that state shows on hover.
  assert.deepEqual(
    seen.rows.map(({ title, status, tip }) => [title, status, tip]),
    [
      // Those that need the user float to the top, newest first like the rest.
      ["Deploy the site", "needsYou:Needs you", "Needs you · approval"],
      ["Broken build", "failed:Failed", "Failed"],
      ["Ship the release", "needsYou:Needs you", "Needs you · sign-in"],
      ["Footer", "working:Working", "Working"],
      ["Header", "working:Working", "Working"],
      ["Fix the login", null, null],
      ["Tidy the docs", null, null],
      ["Waiting on a limit", "working:Working", "Working · waiting for reset 14:05"],
      ["Lone worker", "working:Working", "Working"],
      ["Old fix", null, null],
      ["Finished reply", null, null],
      ["Paused task", "stopped:Stopped", "Stopped"],
      // A finished turn has no word at all; git/PR state never gives it one.
      ["Draft the notes", null, null],
    ],
  );
  for (const row of seen.rows) {
    assert.equal(row.words === null, row.status === null, `${row.title}: a plain state word, none when finished`);
    assert.equal(row.mark, row.status?.startsWith("working:") ? "spinner" : row.status?.startsWith("needsYou:") ? "unread" : null, `${row.title}: its mark`);
    assert.equal(row.error, row.status?.startsWith("failed:") ?? false);
    assert.equal(row.plain, true, `${row.title}: no chip`);
  }
  // The title tells sessions apart: the state takes at most two fifths of the row.
  assert.equal(seen.widths.length, 8, "the eight rows with a state word; finished rows have none");
  for (const row of seen.widths) {
    assert.ok(row.titleShare >= 55, `${row.title}: the title keeps most of the row (${row.titleShare}%)`);
    assert.ok(row.stateWhole, `${row.title}: the state's words show whole`);
  }
  assert.deepEqual(seen.projectHeader, { text: "brigadier", icons: 1 }, "the project header contains only its folder icon and name");
  assert.deepEqual(seen.opened, { type: "conversation", id: "w1-a" }, "the row opens its own session");
  assert.deepEqual(seen.activeAfter, ["Header"]);
  assert.deepEqual(
    seen.rowsAfter,
    ["Deploy the site", "Broken build", "Old fix", "Ship the release", "Footer", "Header", "Fix the login", "Tidy the docs", "Waiting on a limit", "Lone worker", "Finished reply", "Paused task", "Draft the notes"],
    "a session that starts needing the user moves up",
  );
  assert.deepEqual(seen.projectHeaderAfter, seen.projectHeader, "sessions needing the user add no badge to the header");
  // A long title beside its state word ends in "…" before the word, never faded under it (issue 551).
  assert.deepEqual(seen.cutStopped, { overflows: true, ellipsis: "ellipsis", mask: "none", clear: true, word: "Stopped" }, JSON.stringify(seen.cutStopped));
  // A finished row has no word: its long title fades out at the end as usual.
  assert.equal(seen.cut.word, null);
  assert.equal(seen.cut.overflows, true);
  assert.notEqual(seen.cut.mask, "none");
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
  assert.equal(seen.chatTabActions, "hidden", "the spinner holds the ⋯'s place until a hover");
  assert.ok((seen.chatTabStatusShare as number) <= 12, "only a mark takes space beside the title");
  assert.deepEqual(seen.notes, ["Still running. Reopen it from the sidebar."], "closing it leaves a small note");
  assert.equal(seen.dialog, false, "no dialog");
  assert.deepEqual(seen.idleNotes, [], "an idle session closes quietly");
});


for (const kind of ["session", "chat"]) for (const unread of [false, true]) {
  test(`${kind} rows preserve activity, error and unread marks with plain state words (${unread ? "unread" : "read"}); the tab shows the row's own marks without words`, { timeout: 60000 }, async (t) => {
    const seen = JSON.parse(await renderFixturePage(t, `sessions.html?states=1${kind === "chat" ? "&chats=1" : ""}${unread ? "&unread=1" : ""}`, "sessions-result", 20000)) as {
      title: string; rowWords: string | null; state: string; openState: string | null; words: string; tip: string; rowTip: string | null; rowMark: string; mark: string | null; dot: boolean; tabError: boolean; width: number; titleFlex: string; beforeMark: string | null; error: boolean; plain: boolean; unreadSize: number | null; unreadSlot: number | null; spinnerSize: number | null; spinnerSlot: number | null; spinnerDuration: string | null; errorSize: number | null; errorSlot: number | null; leadingError: number | null;
    }[];
    assert.deepEqual([...new Set(seen.map((row) => row.rowWords))].toSorted(), ["Working", "Needs you", "Failed", "Stopped", null].toSorted(), "a finished row has no word");
    // A draft PR does not supply the tab's state word.
    assert.equal(seen.find((row) => row.title === "Draft the notes")?.tip, "Draft the notes");
    for (const row of seen) {
      const busy = row.state === "working";
      assert.equal(row.beforeMark, busy ? "spinner" : row.state === "needsYou" || (row.state === "done" && unread) ? "unread" : null, `${row.title}: closed row mark`);
      // In front of the user, a row shows no Needs you, nor any dot: its spinner if it works.
      assert.notEqual(row.openState, "needsYou", `${row.title}: no Needs you while open`);
      assert.equal(row.rowMark, row.openState === "working" ? "spinner" : null, `${row.title}: an open row has no dot`);
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


test("waiting questions show Needs you and the blue dot, also when an approval waits, but not while open; answers and skips clear it", { timeout: 60000 }, async (t) => {
  type Row = { status: string; tip: string; mark: string | null; active: boolean };
  const seen = JSON.parse(await renderFixturePage(t, "sessions.html?questions=1", "sessions-result", 20000)) as { before: Row[]; opened: Row; after: Row[] };
  for (const row of seen.before) {
    assert.equal(row.status, "needsYou:Needs you");
    assert.ok(["Needs you · question", "Needs you · approval"].includes(row.tip));
    assert.equal(row.mark, "unread");
  }
  // The question is in front of the user: its row says what its turn does instead.
  assert.deepEqual({ status: seen.opened.status, tip: seen.opened.tip, mark: seen.opened.mark }, { status: "working:Working", tip: "Working", mark: "spinner" });
  assert.equal(seen.opened.active, true);
  assert.deepEqual(seen.after.map(({ status, tip, mark }) => ({ status, tip, mark })), [
    { status: "working:Working", tip: "Working", mark: "spinner" },
    { status: "needsYou:Needs you", tip: "Needs you · approval", mark: "unread" },
  ]);
});


test("unopened sessions with working workers keep Working until idle, including reconnect snapshots; user requests still win", { timeout: 60000 }, async (t) => {
  type Row = { title: string; status: string; mark: string | null };
  const seen = JSON.parse(await renderFixturePage(t, "sessions.html?workerActivity=1", "sessions-result", 20000)) as { before: Row[]; someWorking: Row; idle: Row; reconnected: Row; threadReads: string[] };
  assert.deepEqual(seen.before.map(({ title, status, mark }) => [title, status, mark]), [
    ["Approval with workers", "needsYou:Needs you", "unread"],
    ["Question with workers", "needsYou:Needs you", "unread"],
    ["Workers still running", "working:Working", "spinner"],
  ]);
  for (const row of [seen.someWorking, seen.reconnected]) {
    assert.equal(row.status, "working:Working");
    assert.equal(row.mark, "spinner");
  }
  assert.equal(seen.idle.status, null, "no Done word");
  assert.equal(seen.idle.mark, null);
  assert.deepEqual(seen.threadReads, [], "no sidebar session's thread was read");
});


test("a session that never ran has no state word or dot", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "sessions.html?neverRan=1", "sessions-result", 20000)) as { rows: { title: string; status: string | null; mark: string | null }[] };
  const row = seen.rows.find((each) => each.title === "Never ran");
  assert.ok(row);
  assert.equal(row.status, null);
  assert.equal(row.mark, null);
});


test("Done uses the recorded turn-end unread dot: opening clears it through closing, and a later turn restores it", { timeout: 60000 }, async (t) => {
  type Row = { status: string; mark: string | null };
  const seen = JSON.parse(await renderFixturePage(t, "sessions.html?doneRead=1", "sessions-result", 20000)) as { before: Row; afterClose: Row; nextTurn: Row };
  assert.equal(seen.before.status, null, "no Done word");
  assert.equal(seen.before.mark, "unread");
  assert.equal(seen.afterClose.status, null, "no Done word");
  assert.equal(seen.afterClose.mark, null);
  assert.equal(seen.nextTurn.status, null, "no Done word");
  assert.equal(seen.nextTurn.mark, "unread");
});
