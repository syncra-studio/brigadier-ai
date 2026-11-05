import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

type Row = { label: string | null; badge: string | null; time: string | null; labelAboveTitle: boolean; status: string | null; merged: string | null; height: number };
type Card = { title: string | null; status: string | null; project: string | null; model: string | null; folder: string | null; beside: boolean };
type Tab = { text: string; on: boolean; icon: boolean; width: number };
type Seen = {
  sections: { title: string; open: boolean; rows: string[] }[];
  rows: Record<string, Row>;
  dockGap: number;
  archivedHeader: { count: string; open: string; opacity: string };
  titleAttributes: number;
  card: Card; chatCard: Card; projectsCard: Card;
  hoverRow: { card: boolean; time: boolean; chip: boolean; actions: string; pin: boolean };
  oneLineRow: Row;
  pillTone: string; pillAllFailed: { text: string; tone: string };
  archiveAction: { text: string; opacity: string };
  start: string[]; afterTurnStart: string[]; afterTurnEnd: string[]; afterRename: string[]; afterSend: string[];
  afterMerge: { order: string[]; action: string };
  failedStatus: string; pill: string | null; outOfView: Record<string, boolean>; firstJump: boolean; secondJump: boolean;
  archived: string[]; actionAfter: string | null;
  tabsInbox: Tab[]; tabsProjects: Tab[]; mode: string; saved: boolean;
  projectsSections: string[]; projectsPill: boolean; projectsAction: boolean; projectsLabels: number;
};

test("Inbox: Pinned, Open and a folded Archived; rows stay put through a turn and move for the user's message; pill, chip, bulk archive and the tabs", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "inbox.html?drive=1", "inbox-result", 45000)) as Seen;
  assert.deepEqual(seen.sections.map(({ title, open }) => [title, open]), [["Pinned", true], ["Open", true], ["Archived", false]]);
  assert.ok(seen.dockGap >= 0 && seen.dockGap < 60, `Archived docks above the footer (${seen.dockGap}px)`);
  // Folded or open, its count and the way to Settings › Archived show without a hover.
  assert.deepEqual(seen.archivedHeader, { count: "3", open: "Open Settings › Archived", opacity: "1" });

  // A badge, the project and the time over the title; a chat says so in the project's place.
  assert.equal(seen.rows["core-tests"]!.label, "brigadier-ai");
  assert.equal(seen.rows["core-tests"]!.badge, "BA");
  assert.equal(seen.rows["core-tests"]!.time, "1m");
  assert.equal(seen.rows["core-tests"]!.labelAboveTitle, true);
  assert.equal(seen.rows.greeting!.label, "Chat");
  assert.equal(seen.rows.greeting!.badge, "");
  assert.equal(seen.rows["core-tests"]!.height, seen.rows.greeting!.height);

  // The card replaces the native title, on keyboard focus as on hover; a chat has no folder.
  assert.equal(seen.titleAttributes, 0);
  assert.deepEqual(seen.card, { title: "Rate limit the exports", status: "Needs you · approval", project: "SAsteady-app", model: "Opus 5.5", folder: "~/Development/steady-app", beside: true });
  assert.deepEqual(seen.chatCard, { title: "Compare Postgres queue options", status: null, project: "Chat", model: "Opus 5.5", folder: null, beside: true });
  // Hovered, ⋯ and Pin take the time's place; the chip and the card show.
  assert.deepEqual(seen.hoverRow, { card: true, time: false, chip: true, actions: "1", pin: true });

  // The blue ring for waiting, amber for failed; the chip on the title line, always-visible bulk action.
  assert.equal(seen.rows["rate-limit"]!.status, "waiting");
  assert.equal(seen.rows.flaky!.status, "amber");
  assert.equal(seen.rows.greeting!.status, "dot");
  assert.equal(seen.rows["core-tests"]!.status, "working");
  assert.equal(seen.rows.stripe!.merged, "Merged");
  assert.equal(seen.rows.postgres!.merged, null);
  assert.equal(seen.archiveAction.text, "Archive 2 merged");
  assert.equal(seen.archiveAction.opacity, "1", "not only on hover");

  // The order: a turn starting or ending, and a rename, move nothing; the user's message does.
  assert.deepEqual(seen.afterTurnStart, seen.start);
  assert.deepEqual(seen.afterTurnEnd, seen.start);
  assert.deepEqual(seen.afterRename, seen.start);
  assert.notEqual(seen.start[0], "Onboarding email sequence");
  assert.deepEqual(seen.afterSend, ["Onboarding email sequence", ...seen.start.filter((title) => title !== "Onboarding email sequence")]);

  // A merge earns the chip and leaves the row where it is.
  assert.deepEqual(seen.afterMerge.order, seen.afterSend);
  assert.equal(seen.afterMerge.action, "Archive 3 merged");

  // The pill counts waiting and failed rows while one is out of view; each click brings the next one in.
  assert.deepEqual(seen.outOfView, { "rate-limit": true, flaky: true });
  assert.equal(seen.failedStatus, "amber");
  assert.equal(seen.pill, "2 need you");
  assert.equal(seen.pillTone, "waiting", "blue while any of them waits");
  assert.deepEqual(seen.pillAllFailed, { text: "1 needs you", tone: "failed" }, "amber once all of them failed");
  assert.equal(seen.firstJump, true, "the next one down");
  assert.equal(seen.secondJump, true, "then the one after");

  assert.deepEqual(seen.archived.toSorted(), ["hero", "logo", "stripe"]);
  assert.equal(seen.actionAfter, null, "nothing merged is left");

  // Two tabs of one width with their icons; the setting is the daemon's.
  assert.deepEqual(seen.tabsInbox.map(({ text, on, icon }) => [text, on, icon]), [["Inbox", true, true], ["Projects", false, true]]);
  assert.deepEqual(seen.tabsProjects.map(({ text, on }) => [text, on]), [["Inbox", false], ["Projects", true]]);
  assert.equal(seen.tabsInbox[0]!.width, seen.tabsInbox[1]!.width);
  assert.equal(seen.mode, "projects");
  assert.equal(seen.saved, true);
  // Projects: one line with its time, mark and card, under its project; Pinned keeps its two lines; no pill and no bulk action.
  assert.equal(seen.oneLineRow.label, null);
  assert.equal(seen.oneLineRow.time, "1m");
  assert.equal(seen.oneLineRow.status, "working");
  assert.equal(seen.oneLineRow.height, 30);
  assert.equal(seen.projectsCard.project, "BAbrigadier-ai");
  assert.deepEqual(seen.projectsSections, ["Pinned", "Projects", "Chats"]);
  assert.equal(seen.projectsPill, false);
  assert.equal(seen.projectsAction, false);
  assert.equal(seen.projectsLabels, 2, "the two pinned rows");
});

test("⌘⌫ archives picked sidebar rows with Undo, skipping archived ones; right-click menus have no delete; the header's icon opens Settings › Archived", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "inbox.html?view=archived&drive=lifecycle", "inbox-result", 45000));
  assert.deepEqual(seen.liveMenu, ["Pin session", "Rename session…", "Archive session"]);
  assert.deepEqual(seen.archivedMenu, ["Unarchive chat"]);
  assert.deepEqual(seen.mixedMenu, ["Archive (1)", "Unarchive (1)"]);
  assert.deepEqual(seen.archivedIds, ["invoices", "logo"]);
  assert.equal(seen.asked, false);
  assert.equal(seen.undo, true);
  assert.deepEqual(seen.pickedAfter, []);
  assert.equal(seen.archivedAgain, 0);
  assert.equal(seen.askedAfterArchived, false);
  assert.deepEqual(seen.opened, { type: "settings", page: "archived" });
});

test("the attention pill follows layout changes even when the waiting and failed IDs do not change", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "inbox.html?drive=layout", "inbox-result", 45000));
  assert.equal(seen.visibleBefore, true);
  assert.equal(seen.pillBefore, null);
  assert.equal(seen.visibleAfter, false);
  assert.equal(seen.pillAfter, "1 needs you");
  assert.equal(seen.jumped, true);
});

test("Settings pictures share the sidebar choice and the setting appears in search", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "inbox.html?drive=settings", "inbox-result", 45000));
  assert.equal(seen.before, "Inbox");
  assert.equal(seen.projects, "projects");
  assert.equal(seen.inbox, "inbox");
  assert.deepEqual(seen.writes, ["projects", "inbox"]);
  assert.match(seen.searchResult, /Sidebar view/);
  assert.equal(seen.selectedPage.page, "general");
});
