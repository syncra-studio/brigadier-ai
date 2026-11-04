import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

type Row = { label: string | null; labelBelowTitle: boolean; status: string | null; merged: string | null; height: number };
type Seen = {
  sections: { title: string; open: boolean; rows: string[] }[];
  rows: Record<string, Row>;
  dockGap: number;
  archiveAction: { text: string; opacity: string };
  start: string[]; afterTurnStart: string[]; afterTurnEnd: string[]; afterRename: string[]; afterSend: string[];
  afterMerge: { order: string[]; action: string };
  failedStatus: string; pill: string | null; outOfView: Record<string, boolean>; firstJump: boolean; secondJump: boolean;
  archived: string[]; actionAfter: string | null;
  toggleInbox: string; toggleProjects: string; mode: string; saved: boolean;
  projectsSections: string[]; projectsPill: boolean; projectsAction: boolean; projectsLabels: number;
};

test("Inbox: Pinned, Open and a folded Archived; rows stay put through a turn and move for the user's message; pill, chip, bulk archive and the one toggle", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "inbox.html?drive=1", "inbox-result", 45000)) as Seen;
  assert.deepEqual(seen.sections.map(({ title, open }) => [title, open]), [["Pinned", true], ["Open", true], ["Archived", false]]);
  assert.ok(seen.dockGap >= 0 && seen.dockGap < 60, `Archived docks above the footer (${seen.dockGap}px)`);

  // The project has its own line under a session's title; a chat is one line.
  assert.equal(seen.rows["core-tests"]!.label, "brigadier-ai");
  assert.equal(seen.rows["core-tests"]!.labelBelowTitle, true);
  assert.equal(seen.rows.greeting!.label, null);
  assert.ok(seen.rows["core-tests"]!.height > seen.rows.greeting!.height);

  // One amber mark for needs you and failed; the chip on the title line, always-visible bulk action.
  assert.equal(seen.rows["rate-limit"]!.status, "amber");
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

  // The pill counts every amber row while one is out of view; each click brings the next one in.
  assert.deepEqual(seen.outOfView, { "rate-limit": true, flaky: true });
  assert.equal(seen.failedStatus, "amber");
  assert.equal(seen.pill, "2 need you");
  assert.equal(seen.firstJump, true, "the next one down");
  assert.equal(seen.secondJump, true, "then the one after");

  assert.deepEqual(seen.archived.toSorted(), ["hero", "logo", "stripe"]);
  assert.equal(seen.actionAfter, null, "nothing merged is left");

  // One toggle, naming the other view; the setting is the daemon's.
  assert.equal(seen.toggleInbox, "Switch to Projects");
  assert.equal(seen.toggleProjects, "Switch to Inbox");
  assert.equal(seen.mode, "projects");
  assert.equal(seen.saved, true);
  // Projects: one line each, the chip, no pill and no bulk action.
  assert.deepEqual(seen.projectsSections, ["Pinned", "Projects", "Chats"]);
  assert.equal(seen.projectsPill, false);
  assert.equal(seen.projectsAction, false);
  assert.equal(seen.projectsLabels, 0);
});

test("the attention pill follows layout changes even when the amber IDs do not change", { timeout: 90000 }, async (t) => {
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
