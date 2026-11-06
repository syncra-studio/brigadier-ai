import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

type Row = { badge: string | null; icon: boolean; text: string | null; status: string | null; merged: string | null; height: number };
type Card = { title: string | null; status: string | null; project: string | null; model: string | null; folder: string | null; beside: boolean };
type Placed = { projects: string[]; sessions: string[] };
type Lines = Record<string, [boolean, string]>;
type Seen = {
  sections: { title: string; open: boolean; rows: string[] }[];
  rows: Record<string, Row>;
  dockGap: number;
  archivedHeader: { count: string; open: string; opacity: string };
  titleAttributes: number;
  card: Card; chatCard: Card; projectsCard: Card;
  hoverRow: { card: boolean; chip: boolean; actions: string; pin: boolean };
  oneLineRow: Row;
  pillTone: string; pillAllFailed: { text: string; tone: string };
  archiveAction: { text: string; opacity: string };
  start: string[]; afterTurnStart: string[]; afterTurnEnd: string[]; afterRename: string[]; afterSend: string[];
  afterMerge: { order: string[]; action: string };
  failedStatus: string; pill: string | null; outOfView: Record<string, boolean>; firstJump: boolean; secondJump: boolean;
  archived: string[]; actionAfter: string | null;
  menuWords: string[]; addProject: boolean; lineActions: number;
  allOn: { lines: Lines; label: string | null }; allStays: [boolean, string];
  firstBadges: Record<string, string>; slotProblems: string[]; brokenTried: boolean;
  showOrder: string[]; placedBefore: Placed; placedAfterAgent: Placed; placedAfterSend: Placed;
  pickedCount: string;
  picked: { lines: Lines; label: string | null; written: string[]; rows: string[] };
  chatsOff: { lines: Lines; label: string | null; hideChats: boolean; rows: string[] };
  nothing: { label: string | null; view: boolean; empty: string | null; sections: string[] };
  allBack: { lines: Lines; label: string | null; written: [string[], boolean]; rows: number };
  mode: string; saved: boolean;
  projectsSections: string[]; projectsPill: boolean; projectsAction: boolean;
  projectMarks: Record<string, string>; projectButtons: string[];
  groupedFiltered: { sections: string[]; projects: string[]; label: string | null; view: boolean };
};

test("Group by None: Pinned, Open and a folded Archived; rows stay put through a turn and move for the user's message; pill, chip, bulk archive and the View control", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "inbox.html?drive=1", "inbox-result", 45000)) as Seen;
  assert.deepEqual(seen.sections.map(({ title, open }) => [title, open]), [["Pinned", true], ["Open", true], ["Archived", false]]);
  assert.ok(seen.dockGap >= 0 && seen.dockGap < 60, `Archived docks above the footer (${seen.dockGap}px)`);
  // Folded or open, its count and the way to Settings › Archived show without a hover.
  assert.deepEqual(seen.archivedHeader, { count: "3", open: "Open Settings › Archived", opacity: "1" });

  // One line: the project's icon, its initials when it has none or it won't load, a chat's bubble; then the title. No time.
  // The fallback shows first and whenever no image has loaded: never a blank slot, at any change to the page.
  assert.deepEqual(seen.firstBadges, { "core-tests": "BA", flaky: "FR", "rate-limit": "SA" });
  assert.deepEqual(seen.slotProblems, []);
  assert.equal(seen.brokenTried, true, "the broken file was really tried");
  assert.equal(seen.rows["core-tests"]!.icon, true);
  assert.equal(seen.rows["core-tests"]!.badge, "");
  assert.equal(seen.rows["rate-limit"]!.icon, false);
  assert.equal(seen.rows["rate-limit"]!.badge, "SA");
  assert.equal(seen.rows.flaky!.icon, false, "a file that won't load");
  assert.equal(seen.rows.flaky!.badge, "FR");
  assert.equal(seen.rows.greeting!.icon, false);
  assert.equal(seen.rows.greeting!.badge, "");
  assert.equal(seen.rows["core-tests"]!.text, "Fix brigadier-core tests");
  assert.equal(seen.rows["core-tests"]!.height, 36);
  assert.equal(seen.rows.greeting!.height, 36);

  // The card replaces the native title, on keyboard focus as on hover; a chat has no folder.
  assert.equal(seen.titleAttributes, 0);
  assert.deepEqual(seen.card, { title: "Rate limit the exports", status: "Needs you · approval", project: "SAsteady-app", model: "Opus 5.5", folder: "~/Development/steady-app", beside: true });
  assert.deepEqual(seen.chatCard, { title: "Compare Postgres queue options", status: null, project: "Chat", model: "Opus 5.5", folder: null, beside: true });
  // Hovered, only ⋯ shows; the chip and the card too.
  assert.deepEqual(seen.hoverRow, { card: true, chip: true, actions: "1", pin: false });

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

  // View: Group by, then Show with All, each project and Chats; no row actions on its lines.
  assert.deepEqual(seen.menuWords, ["Group by", "Show"]);
  assert.equal(seen.addProject, true);
  assert.equal(seen.lineActions, 0);
  const everything: Lines = { All: [true, "11"], "brigadier-ai": [true, "3"], freelogo: [true, "1"], "steady-app": [true, "3"], "steady-app-billing-service": [true, "No sessions"], Chats: [true, "4"] };
  assert.deepEqual(seen.allOn, { lines: everything, label: null });
  assert.deepEqual(seen.allStays, [true, "11"], "clicking All with everything showing changes nothing");
  // Picking individually unticks All; the label counts projects only.
  assert.deepEqual(seen.picked.lines, { ...everything, All: [false, "11"], freelogo: [false, "1"], "steady-app": [false, "3"], "steady-app-billing-service": [false, "No sessions"] });
  assert.equal(seen.picked.label, "1 project");
  assert.deepEqual(seen.picked.written, ["freelogo", "steady-app", "steady-app-billing-service"]);
  assert.ok(seen.picked.rows.includes("Greeting"), "chats mix in until Chats is unticked");
  assert.ok(!seen.picked.rows.includes("Rate limit the exports"));
  assert.ok(seen.picked.rows.includes("Fix brigadier-core tests"));
  assert.equal(seen.chatsOff.label, "1 project");
  assert.equal(seen.chatsOff.hideChats, true);
  assert.ok(!seen.chatsOff.rows.includes("Greeting"));
  assert.ok(seen.chatsOff.rows.includes("Test"), "Archived stays whole: an archived chat");
  assert.ok(seen.picked.rows.includes("Pricing page copy review"), "Archived stays whole: an unticked project's row");
  assert.equal(seen.pickedCount, "6", "and its count");
  // With nothing to list, Open's header keeps the control.
  assert.deepEqual(seen.nothing, { label: "0 projects", view: true, empty: "Nothing here with this view.", sections: ["Open", "Archived"] });
  assert.deepEqual(seen.allBack.lines, everything, "All returns to everything");
  assert.equal(seen.allBack.label, null);
  assert.deepEqual(seen.allBack.written, [[], false]);

  // Group by Project is today's Projects, with no pill or bulk action; a project's icon, else its folder.
  assert.equal(seen.mode, "projects");
  assert.equal(seen.saved, true);
  assert.equal(seen.oneLineRow.badge, null);
  assert.equal(seen.oneLineRow.text, "Fix brigadier-core tests");
  assert.equal(seen.oneLineRow.status, "working");
  assert.equal(seen.oneLineRow.height, 30);
  assert.equal(seen.projectsCard.project, "brigadier-ai");
  assert.deepEqual(seen.projectsSections, ["Pinned", "Projects", "Chats"]);
  assert.equal(seen.projectsPill, false);
  assert.equal(seen.projectsAction, false);
  assert.deepEqual(seen.projectMarks, { "brigadier-ai": "icon", freelogo: "folder", "steady-app": "folder", "steady-app-billing-service": "folder" });
  assert.deepEqual(seen.projectButtons, ["Actions for brigadier-ai", "New session in brigadier-ai"]);
  // Projects stay in name order, in the list and in Show; only the user's message moves a session.
  const byName = ["brigadier-ai", "freelogo", "steady-app", "steady-app-billing-service"];
  assert.deepEqual(seen.showOrder, ["All", ...byName, "Chats"]);
  assert.deepEqual(seen.placedBefore.projects, byName);
  // A row that needs the user (Rate limit the exports) has no place of its own.
  assert.deepEqual(seen.placedBefore.sessions, ["Onboarding email sequence", "Rate limit the exports", "Invoice export columns"]);
  assert.deepEqual(seen.placedAfterAgent, seen.placedBefore, "an agent's update moves neither a project nor a session");
  assert.deepEqual(seen.placedAfterSend, { projects: byName, sessions: ["Invoice export columns", "Onboarding email sequence", "Rate limit the exports"] }, "the user's message moves its session, not its project");
  // Show narrows it too, the Chats section with it; the control stays.
  assert.deepEqual(seen.groupedFiltered.sections, ["Pinned", "Projects"]);
  assert.ok(!seen.groupedFiltered.projects.includes("freelogo"));
  assert.equal(seen.groupedFiltered.label, "3 projects");
  assert.equal(seen.groupedFiltered.view, true);
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
  assert.equal(seen.before, "One list");
  assert.equal(seen.projects, "projects");
  assert.equal(seen.inbox, "inbox");
  assert.deepEqual(seen.writes, ["projects", "inbox"]);
  assert.match(seen.searchResult, /Sidebar view/);
  assert.equal(seen.selectedPage.page, "general");
});
