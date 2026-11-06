/**
 * The real sidebar with Group by None over a faked daemon, with the rows of the approved mockups.
 * ?view=archived opens the Archived section, ?view=projects starts with Group by Project,
 * ?view=settings shows Settings › General; ?show=picked unticks all but brigadier-ai, and Chats;
 * ?open=view or ?open=menu opens the View control or a row's ⋯; ?archived=101 lists that many
 * archived rows. brigadier-ai has an icon, freelogo's won't load,
 * steady-app has none. The page's main area has buttons to start and end a turn and to send a
 * message, through the same events the daemon sends, to see which rows move. ?drive=1 goes through
 * all of it and writes what it saw to #inbox-result (InboxRender.test.ts reads it).
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { SettingsNav } from "@/app/settings/SettingsNav";
import { SettingsView } from "@/app/settings/SettingsView";
import { useLifecycleShortcuts } from "@/app/lifecycleShortcuts";
import { AppSidebar, AppStrip } from "@/app/shell/AppSidebar";
import { SidebarFoot } from "@/app/shell/SidebarFoot";
import { SidebarPanel, SidebarProvider } from "@/components/layout";
import { Toaster, TooltipProvider } from "@/components/overlays";
import type { Conversation, ConversationActivity, DomainEvent, EventEnvelope, ModelChoice, Project, Request } from "@/ipc/generated";
import { fixtureClick, fixtureInput } from "@/fixtures/input";
import { openSettings } from "@/state/actions";
import { usePicked } from "@/state/picking";
import { fromSnapshot, useActivity } from "@/state/activity";
import { applyEvents, useApp } from "@/state/store";
import { applySettledTabEvents, useSettledTabs } from "@/state/tabStates";

const params = new URLSearchParams(location.search);
const view = params.get("view");
const requests: Request[] = [];
const ICONS: Record<string, string> = {
  "brigadier-ai": "data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAzMiAzMiI+PHJlY3Qgd2lkdGg9IjMyIiBoZWlnaHQ9IjMyIiByeD0iNyIgZmlsbD0iIzNiODJmNiIvPjxwYXRoIGQ9Ik05IDIyIDE2IDhsNyAxNGgtNWwtMi00LTIgNHoiIGZpbGw9IiNmZmYiLz48L3N2Zz4=",
  freelogo: "data:image/png;base64,bm90IGFuIGltYWdl",
};
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  requests.push(req);
  if (req.method === "projectIcon") return { method: req.method, icon: ICONS[req.projectId] ?? null };
  if (req.method === "updateSettings") return { method: req.method, settings: req.settings };
  if (req.method === "archive") return { method: req.method, outcomes: req.ids.map((id) => ({ id, conversation: { ...useApp.getState().conversations[id], lifecycle: "archived" } })) };
  return { method: req.method };
});

const NOW = Date.now();
const MIN = 60_000;
// No providers are listed here, so a model shows by its id: this one reads as its name would.
const OPUS = { provider: "claude", model: "Opus 5.5", effort: null } as unknown as ModelChoice;
const session = (id: string, title: string, projectId: string, age: number, extra: Partial<Conversation> = {}): Conversation => ({
  id, kind: "session", projectId, title, pinnedAtMs: null, createdAtMs: NOW - 1000 * MIN, updatedAtMs: NOW - age * MIN, lastSentAtMs: NOW - age * MIN,
  setup: { type: "session", repo: `/Users/stephen/Development/${projectId}`, environment: { type: "local" }, permission: "default", orchestrator: OPUS, workersSeeUncommitted: null, planMode: false } as unknown as Conversation["setup"], lifecycle: "active", forkedFrom: null, sideOf: null, quotaWait: null, seenAtMs: NOW, ...extra,
});
const chat = (id: string, title: string, age: number, extra: Partial<Conversation> = {}): Conversation =>
  session(id, title, "", age, { kind: "chat", projectId: null, setup: { type: "chat", model: OPUS, permission: "default" } as unknown as Conversation["setup"], ...extra });
const unread = { finishedAtMs: NOW, seenAtMs: 0 };

const conversations = [
  session("routing", "Regarding our routing page", "brigadier-ai", 500, { pinnedAtMs: 2 }),
  chat("release", "Release notes draft", 600, { pinnedAtMs: 1 }),
  session("core-tests", "Fix brigadier-core tests", "brigadier-ai", 1),
  session("rate-limit", "Rate limit the exports", "steady-app", 2),
  chat("greeting", "Greeting", 3, unread),
  session("dev-build", "In brigadier-ai, a dev build", "brigadier-ai", 4),
  session("hero", "Landing page hero copy", "freelogo", 5, unread),
  session("stripe", "Stripe webhook retries", "steady-app-billing-service", 6),
  chat("postgres", "Compare Postgres queue options", 7),
  session("logo", "Logo picker fixes", "freelogo", 8),
  session("onboarding", "Onboarding email sequence", "steady-app", 9),
  chat("changelog", "Tidy the changelog", 10),
  session("invoices", "Invoice export columns", "steady-app", 11),
  session("flaky", "Flaky login test on CI", "freelogo", 12),
  chat("test", "Test", 700, { lifecycle: "archived", updatedAtMs: NOW - MIN }),
  session("tauri", "Upgrade Tauri to the new runtime", "brigadier-ai", 800, { lifecycle: "archived", updatedAtMs: NOW - 2 * MIN }),
  session("pricing", "Pricing page copy review", "freelogo", 900, { lifecycle: "archived", updatedAtMs: NOW - 3 * MIN }),
  ...Array.from({ length: Math.max(0, Number(params.get("archived") ?? 0) - 3) }, (_, index) =>
    session(`old-${index}`, `Older archived session ${index + 1}`, "steady-app", 1000 + index * 240, { lifecycle: "archived" })),
];
const project = (id: string, age: number) => [id, { id, name: id, repos: [], createdAtMs: NOW - age } as unknown as Project] as const;
useApp.setState((state) => ({
  catalogLoaded: true,
  connection: { status: "connected", daemon: null, reason: null },
  info: { platform: "macos", version: "fixture", processStartMs: 0, smoke: false, firstLaunch: false, budgetTolerance: 1 },
  projects: Object.fromEntries([project("brigadier-ai", 1), project("freelogo", 2), project("steady-app", 3), project("steady-app-billing-service", 4)]),
  conversations: Object.fromEntries(conversations.map((each) => [each.id, each])),
  selection: { type: "conversation", id: "core-tests" },
  expandedProjects: { "steady-app-billing-service": false },
  settings: { ...state.settings, userName: "Stephen", sidebarMode: view === "projects" ? "projects" : "inbox", ...(params.get("show") === "picked" ? { sidebarHiddenProjects: ["freelogo", "steady-app", "steady-app-billing-service"], sidebarHideChats: true } : {}) },
}));
const activity = (id: string, extra: Partial<ConversationActivity>): [string, ReturnType<typeof fromSnapshot>] =>
  [id, fromSnapshot({ conversationId: id, run: "idle", tasks: [], approvals: [], questions: [], ...extra })];
const working = (id: string) => activity(id, { run: "running", turnOpen: true });
const asking = (id: string) => activity(id, { approvals: [[`${id}-card`, "pending"]] } as unknown as Partial<ConversationActivity>);
const failed = (id: string) => activity(id, { failed: true });
useActivity.setState({
  byConversation: Object.fromEntries([working("core-tests"), working("dev-build"), asking("rate-limit"), failed("flaky")]),
});
useSettledTabs.setState({ byTab: { stripe: { prMerged: true }, logo: { prMerged: true } } });
if (view === "archived") localStorage.setItem("brigadier.inboxFolded", "[]");
else localStorage.removeItem("brigadier.inboxFolded");
if (view === "settings") openSettings("general");

let seq = 1000;
const event = (stream: string, domain: DomainEvent): EventEnvelope =>
  ({ seq: seq++, stream, streamSeq: seq, atMs: Date.now(), event: domain }) as unknown as EventEnvelope;
const setActivity = (entry: [string, ReturnType<typeof fromSnapshot>]) =>
  useActivity.setState((state) => ({ byConversation: { ...state.byConversation, [entry[0]]: entry[1] } }));

/** What the daemon sends as a turn starts, and as it ends. */
const startTurn = (id: string) => setActivity(working(id));
const endTurn = (id: string) => {
  setActivity(activity(id, { turnEndedAtMs: Date.now() }));
  applyEvents([event(`conversation:${id}`, { type: "requestFinished", conversationId: id, requestId: "r" } as DomainEvent)]);
  useApp.setState((state) => {
    const row = state.conversations[id]!;
    return { conversations: { ...state.conversations, [id]: { ...row, finishedAtMs: Date.now(), updatedAtMs: Date.now() } } };
  });
};
/** What the daemon sends when the user's message goes out. */
const userSends = (id: string) => applyEvents([event("catalog", { type: "conversationSent", id, atMs: Date.now() })]);

const openRows = () => [...document.querySelectorAll<HTMLElement>('section')].find((section) => section.querySelector("button[aria-expanded]")?.textContent?.trim() === "Open");
const titles = () => [...(openRows()?.querySelectorAll<HTMLElement>('[data-slot="row-title"]') ?? [])].map((title) => title.textContent);
const rowTitles = () => [...document.querySelectorAll<HTMLElement>('[data-row-id] [data-slot="row-title"]')].map((title) => title.textContent);
const projectRows = () => [...document.querySelectorAll<HTMLElement>("section li > div > button[aria-expanded]:not([aria-haspopup])")];
const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const until = async (check: () => boolean) => { for (let n = 0; n < 200 && !check(); n++) await wait(20); };

const q = <T extends HTMLElement = HTMLElement>(selector: string, root: ParentNode = document) => root.querySelector<T>(selector);

async function drive(): Promise<Record<string, unknown>> {
  const seen: Record<string, unknown> = {};
  const sections = () => [...document.querySelectorAll<HTMLElement>("section")].map((section) => ({
    title: q("button[aria-expanded]", section)?.textContent?.trim(),
    open: q("button[aria-expanded]", section)?.getAttribute("aria-expanded") === "true",
    rows: [...section.querySelectorAll<HTMLElement>('[data-slot="row-title"]')].map((title) => title.textContent),
  }));
  seen.sections = sections();
  // freelogo's icon has had its time to fail.
  await until(() => q('[data-slot="row-badge"]', q('[data-row-id="flaky"]')!)?.textContent === "FR");
  const row = (id: string) => q(`[data-row-id="${id}"]`)!;
  const rowOf = (id: string) => ({
    badge: q('[data-slot="row-badge"]', row(id))?.textContent ?? null,
    icon: q('[data-slot="project-icon"]', row(id)) !== null,
    text: q("button", row(id))?.textContent ?? null,
    status: q('[data-slot="row-status"]', row(id))?.dataset.status ?? null,
    merged: q('[data-slot="row-merged"]', row(id))?.textContent ?? null,
    height: Math.round(row(id).getBoundingClientRect().height),
  });
  seen.rows = Object.fromEntries(["core-tests", "rate-limit", "greeting", "hero", "stripe", "postgres", "flaky"].map((id) => [id, rowOf(id)]));
  // The dock sits right above the footer, and the list runs down to it.
  const dock = q('[data-slot="inbox-archived"]')!.getBoundingClientRect();
  const panel = q('[data-slot="sidebar-panel"]')!.getBoundingClientRect();
  seen.dockGap = Math.round(panel.bottom - dock.bottom);
  seen.archivedHeader = {
    count: q('[data-slot="archived-count"]')?.textContent,
    open: q('[data-slot="open-archived"]')?.getAttribute("aria-label"),
    opacity: getComputedStyle(q('[data-slot="open-archived"]')!.parentElement!).opacity,
  };
  seen.titleAttributes = [...document.querySelectorAll('[data-row-id] [title]')].length;
  seen.card = await cardOf("rate-limit");
  seen.chatCard = await cardOf("postgres");
  seen.hoverRow = await hoverRow("stripe");
  seen.archiveAction = { text: q('[data-slot="archive-merged"]')?.textContent, opacity: getComputedStyle(q('[data-slot="archive-merged"]')!.parentElement!).opacity };

  // Rows stay put through a turn, and move for the user's message.
  const start = titles();
  seen.start = start;
  startTurn("onboarding");
  await wait(50);
  seen.afterTurnStart = titles();
  endTurn("onboarding");
  await wait(2500);
  seen.afterTurnEnd = titles();
  applyEvents([event("catalog", { type: "conversationRenamed", id: "postgres", title: "Compare Postgres queue options" })]);
  await wait(50);
  seen.afterRename = titles();
  userSends("onboarding");
  await until(() => titles()[0] === "Onboarding email sequence");
  seen.afterSend = titles();
  // A pull request the board sees merged earns its chip, in place.
  applySettledTabEvents([event("conversation:hero", { type: "pullRequestSeen", conversationId: "hero", pullRequest: { number: 1, title: "t", url: "u", state: "merged" } } as unknown as DomainEvent)]);
  await until(() => q('[data-slot="row-merged"]', row("hero")) !== null);
  seen.afterMerge = { order: titles(), action: q('[data-slot="archive-merged"]')?.textContent };

  // A failed row has the amber mark and counts in the pill too.
  seen.failedStatus = q('[data-slot="row-status"]', row("flaky"))?.dataset.status;
  // The pill, with a row scrolled out of view; a click brings it in.
  q<HTMLElement>(".h-screen")!.classList.replace("h-screen", "h-96");
  const list = q<HTMLElement>(".scroll-edge-fade")!;
  list.scrollTop = 0;
  await wait(100);
  const pill = () => q('[data-slot="need-you-pill"]');
  seen.pill = pill()?.textContent ?? null;
  seen.pillTone = pill()?.getAttribute("data-tone");
  const inView = (id: string) => { const box = row(id).getBoundingClientRect(), of = list.getBoundingClientRect(); return box.top >= of.top && box.bottom <= of.bottom; };
  seen.outOfView = { "rate-limit": !inView("rate-limit"), flaky: !inView("flaky") };
  pill()?.click();
  await until(() => inView("rate-limit"));
  await wait(600);
  seen.firstJump = inView("rate-limit");
  pill()?.click();
  await until(() => inView("flaky"));
  seen.secondJump = inView("flaky");
  // With nothing waiting, what's left is amber.
  setActivity(activity("rate-limit", {}));
  list.scrollTop = 0;
  await until(() => pill()?.getAttribute("data-tone") === "failed");
  seen.pillAllFailed = { text: pill()?.textContent, tone: pill()?.getAttribute("data-tone") };
  setActivity(asking("rate-limit"));

  // One click archives the merged rows; Archived then holds them, first of all.
  const archiveRequest = () => requests.find((request) => request.method === "archive");
  q('[data-slot="archive-merged"]')!.click();
  await until(() => archiveRequest() !== undefined);
  await wait(100);
  seen.archived = (archiveRequest() as { ids?: string[] } | undefined)?.ids;
  seen.actionAfter = q('[data-slot="archive-merged"]')?.textContent ?? null;

  // View: Group by, and Show with All, each project and Chats.
  q<HTMLElement>(".h-96")!.classList.replace("h-96", "h-screen");
  const viewButton = () => q('[data-slot="sidebar-view"]');
  const label = () => q('[data-slot="view-filtered"]')?.textContent ?? null;
  const box = (name: string) => q<HTMLElement>(`[data-slot="sidebar-view-menu"] [aria-label="${name}"]`)!;
  const lines = () => Object.fromEntries([...document.querySelectorAll<HTMLElement>('[data-slot="show-row"]')].map((line) => [
    q('[role="checkbox"]', line)!.getAttribute("aria-label"),
    [q('[role="checkbox"]', line)!.getAttribute("aria-checked") === "true", q('[data-slot="show-count"]', line)!.textContent],
  ]));
  const click = async (element: HTMLElement) => { element.click(); await wait(60); };
  await click(viewButton()!);
  await until(() => q('[data-slot="sidebar-view-menu"]') !== null);
  seen.menuWords = [...q('[data-slot="sidebar-view-menu"]')!.querySelectorAll("div")].filter((part) => part.childElementCount === 0).map((part) => part.textContent).filter(Boolean);
  seen.addProject = [...q('[data-slot="sidebar-view-menu"]')!.querySelectorAll("button")].some((button) => button.textContent === "Add project…");
  seen.lineActions = document.querySelectorAll('[data-slot="show-row"] [aria-label^="Actions for"]').length;
  seen.allOn = { lines: lines(), label: label() };
  // All stays ticked when clicked with everything showing.
  await click(box("All"));
  seen.allStays = lines().All;
  await click(box("freelogo"));
  await click(box("steady-app"));
  await click(box("steady-app-billing-service"));
  seen.picked = { lines: lines(), label: label(), written: useApp.getState().settings.sidebarHiddenProjects, rows: rowTitles() };
  seen.pickedCount = q('[data-slot="archived-count"]')?.textContent;
  await click(box("Chats"));
  seen.chatsOff = { lines: lines(), label: label(), hideChats: useApp.getState().settings.sidebarHideChats, rows: rowTitles() };
  await click(box("brigadier-ai"));
  seen.nothing = { label: label(), view: viewButton() !== null, empty: q("section li")?.textContent ?? null, sections: sections().map((section) => section.title) };
  await click(box("brigadier-ai"));
  await click(box("All"));
  seen.allBack = { lines: lines(), label: label(), written: [useApp.getState().settings.sidebarHiddenProjects, useApp.getState().settings.sidebarHideChats], rows: rowTitles().length };

  // Group by Project: today's Projects, under the same control.
  await click([...document.querySelectorAll<HTMLElement>('[data-slot="radio"]')].at(-1)!);
  await until(() => Boolean(q('button[aria-label="Add project"]')));
  seen.oneLineRow = rowOf("core-tests");
  seen.mode = useApp.getState().settings.sidebarMode;
  seen.saved = requests.some((request) => request.method === "updateSettings");
  seen.projectsSections = sections().map((section) => section.title);
  seen.projectsPill = pill() !== null;
  seen.projectsAction = q('[data-slot="archive-merged"]') !== null;
  seen.projectMarks = Object.fromEntries(projectRows().map((button) => [button.textContent?.trim(), button.querySelector("img") ? "icon" : button.querySelector("svg") ? "folder" : null]));
  seen.projectButtons = [...document.querySelectorAll('li > div button[aria-label$=" brigadier-ai"]')].map((button) => button.getAttribute("aria-label"));
  await click(box("freelogo"));
  await click(box("Chats"));
  seen.groupedFiltered = { sections: sections().map((section) => section.title), projects: projectRows().map((button) => button.textContent?.trim()), label: label(), view: viewButton() !== null };
  await click(box("All"));
  await fixtureInput({ type: "key", key: "Escape" });
  await until(() => q('[data-slot="sidebar-view-menu"]') === null);
  seen.projectsCard = await cardOf("core-tests");
  return seen;
}

/** The card a row shows on keyboard focus, then what shows on the row's hover. */
async function cardOf(id: string): Promise<Record<string, unknown>> {
  const button = q<HTMLButtonElement>(`[data-row-id="${id}"] button`)!;
  const card = () => q('[data-slot="row-card"]');
  button.focus();
  await until(() => card() !== null);
  const slot = (name: string) => q(`[data-slot="${name}"]`, card() ?? document)?.textContent ?? null;
  const seen = {
    title: slot("card-title"), status: slot("card-status"), project: slot("card-project"), model: slot("card-model"), folder: slot("card-folder"),
    beside: card() ? card()!.getBoundingClientRect().left >= button.getBoundingClientRect().right : false,
  };
  button.blur();
  await until(() => card() === null);
  return seen;
}

const shown = (element: Element | null) => element !== null && getComputedStyle(element).display !== "none" && getComputedStyle(element).visibility !== "hidden";

/** A row under the real pointer: the card, and what it keeps. */
async function hoverRow(id: string): Promise<Record<string, unknown>> {
  const row = q<HTMLElement>(`[data-row-id="${id}"]`)!;
  const box = row.getBoundingClientRect();
  await fixtureInput({ type: "pointer", x: box.x + box.width / 2, y: box.y + box.height / 2 });
  await until(() => q('[data-slot="row-card"]') !== null);
  const seen = {
    card: q('[data-slot="row-card"]') !== null,
    chip: shown(q('[data-slot="row-merged"]', row)),
    actions: getComputedStyle(q<HTMLElement>(`button[aria-label^="Actions for"]`, row)!.parentElement!).opacity,
    pin: q(`button[aria-label^="Pin"]`, row) !== null,
  };
  await fixtureInput({ type: "pointer", x: 1200, y: 900 });
  await until(() => q('[data-slot="row-card"]') === null);
  return seen;
}

/**
 * ⌘⌫ on picked sidebar rows archives those not archived yet, with the usual Undo, and asks nothing;
 * the rows' right-click menus have no delete, one row or several.
 */
async function driveLifecycle(): Promise<Record<string, unknown>> {
  const seen: Record<string, unknown> = {};
  const contextItems = async (id: string) => {
    const box = q(`[data-row-id="${id}"] button`)!;
    box.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, clientX: 40, clientY: 200 }));
    await until(() => q('[role="menu"]') !== null);
    const items = [...document.querySelectorAll('[role="menu"] [role="menuitem"]')].map((item) => item.textContent?.trim());
    await fixtureInput({ type: "key", key: "Escape" });
    await until(() => q('[role="menu"]') === null);
    return items;
  };
  seen.liveMenu = await contextItems("logo");
  seen.archivedMenu = await contextItems("test");
  usePicked.setState({ list: "sidebar", ids: ["invoices", "test"], anchor: "invoices" });
  await wait(50);
  seen.mixedMenu = await contextItems("invoices");
  usePicked.setState({ list: "sidebar", ids: ["invoices", "logo", "test"], anchor: "invoices" });
  await fixtureInput({ type: "key", key: "Backspace", modifiers: ["Meta"] });
  await until(() => requests.some((request) => request.method === "archive"));
  seen.archivedIds = (requests.find((request) => request.method === "archive") as { ids?: string[] } | undefined)?.ids;
  seen.asked = q('[role="dialog"]') !== null;
  seen.undo = [...document.querySelectorAll("button")].some((button) => button.textContent?.trim() === "Undo");
  seen.pickedAfter = usePicked.getState().ids;
  // A pick of archived rows alone: nothing to archive, nothing asked.
  usePicked.setState({ list: "sidebar", ids: ["test"], anchor: "test" });
  const before = requests.filter((request) => request.method === "archive").length;
  await fixtureInput({ type: "key", key: "Backspace", modifiers: ["Meta"] });
  await wait(100);
  seen.archivedAgain = requests.filter((request) => request.method === "archive").length - before;
  seen.askedAfterArchived = q('[role="dialog"]') !== null;
  usePicked.setState({ list: null, ids: [], anchor: null });
  // The header's icon goes to the Settings page.
  q('[data-slot="open-archived"]')!.click();
  await until(() => useApp.getState().selection.type === "settings");
  seen.opened = useApp.getState().selection;
  return seen;
}

/** Reordering another row must refresh the pill even when the amber IDs stay the same. */
async function driveLayout(): Promise<Record<string, unknown>> {
  q<HTMLElement>(".h-screen")!.classList.replace("h-screen", "h-120");
  setActivity(activity("flaky", {}));
  await wait(150);
  const list = q<HTMLElement>(".scroll-edge-fade")!;
  const row = () => q<HTMLElement>('[data-row-id="rate-limit"]')!;
  row().scrollIntoView({ block: "center" });
  await wait(150);
  const inView = () => { const box = row().getBoundingClientRect(), viewport = list.getBoundingClientRect(); return box.top >= viewport.top && box.bottom <= viewport.bottom; };
  const visibleBefore = inView();
  const pillBefore = q('[data-slot="need-you-pill"]')?.textContent ?? null;
  // Same rows, same container size, same amber ID: ten own sends move other rows above it.
  for (const id of ["greeting", "dev-build", "hero", "stripe", "postgres", "logo", "onboarding", "changelog", "invoices", "flaky"]) userSends(id);
  await wait(200);
  const visibleAfter = inView();
  const pillAfter = q('[data-slot="need-you-pill"]')?.textContent ?? null;
  q('[data-slot="need-you-pill"]')?.click();
  await until(inView);
  return { visibleBefore, pillBefore, visibleAfter, pillAfter, jumped: inView() };
}

async function driveSettings(): Promise<Record<string, unknown>> {
  openSettings("general");
  await until(() => q('[aria-label="Sidebar view"]') !== null);
  const choice = () => q('[aria-label="Sidebar view"]')!;
  const active = () => q('[data-state="on"]', choice())?.textContent;
  const before = active();
  await fixtureClick([...choice().querySelectorAll("button")].find((button) => button.textContent === "By project")!);
  await until(() => active() === "By project");
  const projects = useApp.getState().settings.sidebarMode;
  await fixtureClick([...choice().querySelectorAll("button")].find((button) => button.textContent === "One list")!);
  await until(() => active() === "One list");
  const inbox = useApp.getState().settings.sidebarMode;
  q<HTMLInputElement>('[aria-label="Search settings"]')!.focus();
  await fixtureInput({ type: "type", text: "Sidebar view" });
  await wait(100);
  const searchResult = [...document.querySelectorAll('button')].find((button) => button.textContent?.includes("Sidebar view") && !button.closest("main"));
  searchResult?.click();
  await wait(100);
  return { before, projects, inbox, writes: requests.filter((request) => request.method === "updateSettings").map((request) => request.settings.sidebarMode), searchResult: searchResult?.textContent, selectedPage: useApp.getState().selection };
}

function Controls() {
  return (
    <div className="text-muted-foreground flex max-w-md flex-col items-start gap-2 p-6 text-sm">
      <p>Try it on a row: only your own message moves it.</p>
      {[
        ["Start a turn on Onboarding email sequence", () => startTurn("onboarding")],
        ["End that turn", () => endTurn("onboarding")],
        ["Send Onboarding email sequence a message", () => userSends("onboarding")],
        ["Send Logo picker fixes a message", () => userSends("logo")],
      ].map(([label, run]) => (
        <button key={label as string} type="button" className="border-border hover:bg-accent text-foreground rounded-md border px-2 py-1" onClick={run as () => void}>
          {label as string}
        </button>
      ))}
    </div>
  );
}

function Fixture() {
  useLifecycleShortcuts();
  const page = useApp((s) => (s.selection.type === "settings" ? s.selection.page : null));
  const [result, setResult] = useState<string | null>(null);
  // ?open=view or ?open=menu opens the View control or the first open row's ⋯, for a look.
  useEffect(() => {
    const target = params.get("open") === "view" ? q('[data-slot="sidebar-view"]') : params.get("open") === "menu" ? q('[data-row-id="core-tests"] button[aria-label^="Actions for"]') : null;
    if (!target) return;
    const timer = setTimeout(() => {
      target.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, button: 0, ctrlKey: false }));
      target.click();
    }, 200);
    return () => clearTimeout(timer);
  }, []);
  useEffect(() => {
    if (!params.has("drive")) return;
    const timer = setTimeout(() => void (params.get("drive") === "layout" ? driveLayout() : params.get("drive") === "settings" ? driveSettings() : params.get("drive") === "lifecycle" ? driveLifecycle() : drive()).then((seen) => setResult(JSON.stringify(seen))), 300);
    return () => clearTimeout(timer);
  }, []);
  return (
    <TooltipProvider>
      <div className="bg-chrome flex h-screen flex-col">
        <SidebarProvider className="relative min-h-0 flex-1 flex-col">
          <div className="relative flex min-h-0 min-w-0 flex-1">
            <SidebarPanel strip={page ? null : <AppStrip />} foot={page ? null : <SidebarFoot />}>
              {page ? <SettingsNav /> : <AppSidebar />}
            </SidebarPanel>
            <main className="bg-background min-w-0 flex-1">{page ? <SettingsView page={page} /> : <Controls />}</main>
          </div>
        </SidebarProvider>
      </div>
      <Toaster className="top-2" />
      {result && <pre id="inbox-result">{result}</pre>}
    </TooltipProvider>
  );
}

const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
