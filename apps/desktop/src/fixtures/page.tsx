import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { PageTabView } from "@/app/conversation/pages/PageTabView";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { Conversation, Page, PageContent } from "@/ipc/generated";
import { emitPageChanged } from "@/state/pageChanges";
import { sessionTabs, useSessionTabs } from "@/state/sessionTabs";
import { useApp } from "@/state/store";

/**
 * A page tab over a daemon kept in memory. `?drive=1` types into it the way a person would
 * (Markdown shortcuts, `/`, selecting and formatting, dragging a block, the File menu) and
 * prints what it saw into #page-result.
 */

const ID = "page-fixture";
const params = new URLSearchParams(location.search);
const pages = new Map<string, Page>();
let revision = 0;
const put = (id: string, content: PageContent): Page => {
  const page = { id, content, revision: `r${++revision}`, createdMs: 0, updatedMs: Date.now() };
  pages.set(id, page);
  return page;
};
const FIRST = "1".repeat(32);
const OTHER = "2".repeat(32);
put(FIRST, { title: "", emoji: null, body: "" });
put(OTHER, { title: "Older notes", emoji: "📌", body: "Kept" });
const calls: { command: string; payload: unknown }[] = [];
const copied: string[] = [];
Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: async (text: string) => { copied.push(text); } } });

const conversation: Conversation = { id: ID, kind: "session", projectId: "fixture-project", title: "Page", pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0, setup: null, lifecycle: "active", forkedFrom: null, sideOf: null, fallback: null, quotaWait: null };
useApp.setState(() => ({
  conversations: { [ID]: conversation }, selection: { type: "conversation", id: ID },
  info: { platform: "macos", version: "fixture", processStartMs: 0, smoke: false, firstLaunch: false, budgetTolerance: 1 },
}));
useSessionTabs.setState({ sessions: { [ID]: { tabs: [{ kind: "page", id: "page:fixture", opener: "chat", pageId: FIRST, title: "" }], active: "page:fixture" } } });

mockIPC((command, payload) => {
  calls.push({ command, payload });
  if (command === "save_document") return `/tmp/${String((payload as { name: string }).name)}`;
  if (command !== "ipc_request") return null;
  const request = (payload as { request: { method: string; id?: string; content?: PageContent; baseRevision?: string } }).request;
  const page = request.id ? pages.get(request.id) : undefined;
  switch (request.method) {
    case "pageRead": if (!page) throw new Error("No such page"); return { method: "pageRead", page };
    case "pageWrite":
      if (!page) throw new Error("No such page");
      if (page.revision !== request.baseRevision) return { method: "pageWrite", outcome: { type: "conflict", current: page } };
      return { method: "pageWrite", outcome: { type: "saved", revision: put(page.id, request.content!).revision } };
    case "pageList": return { method: "pageList", pages: [...pages.values()].toSorted((a, b) => b.updatedMs - a.updatedMs).map((entry) => ({ id: entry.id, title: entry.content.title, emoji: entry.content.emoji, updatedMs: entry.updatedMs })) };
    case "pageDelete": pages.delete(request.id!); return { method: "pageDelete" };
    case "pageCreate": return { method: "pageCreate", page: put(crypto.randomUUID().replaceAll("-", ""), { title: "", emoji: null, body: "" }) };
    default: return null;
  }
});

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const until = async (check: () => boolean, ms = 3000) => { for (const end = Date.now() + ms; Date.now() < end && !check();) await wait(25); };
const one = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector);
const content = () => one<HTMLElement>(".page-content")!;
const type = (text: string) => document.execCommand("insertText", false, text);
const enter = () => content().dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", code: "Enter", bubbles: true, cancelable: true }));
const blocks = () => [...content().children].map((block) => block.tagName.toLowerCase() + (block.className.includes("page-checklist") ? ".check" : ""));
const box = (element: Element | null) => { const rect = element?.getBoundingClientRect(); return rect ? { width: Math.round(rect.width), height: Math.round(rect.height) } : null; };
const menuItems = () => [...document.querySelectorAll<HTMLElement>('[role="menu"][data-state="open"] [role="menuitem"]')];
const openMenu = (label: string) => {
  const trigger = [...document.querySelectorAll<HTMLElement>("button")].find((button) => button.getAttribute("aria-label") === label || button.textContent?.trim() === label);
  trigger?.focus();
  trigger?.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
};
/** Puts the caret at the end of the page's last block. */
const caretAtEnd = () => {
  const last = content().lastElementChild!;
  const range = document.createRange();
  range.selectNodeContents(last);
  range.collapse(false);
  getSelection()!.removeAllRanges();
  getSelection()!.addRange(range);
};

async function drive(): Promise<Record<string, unknown>> {
  const seen: Record<string, unknown> = {};
  try { await steps(seen); } catch (error) { seen.error = String(error); }
  return seen;
}

async function steps(seen: Record<string, unknown>): Promise<void> {
  await until(() => one(".page-content") !== null && one<HTMLTextAreaElement>(".page-title") === document.activeElement);
  const title = one<HTMLTextAreaElement>(".page-title")!;
  seen.newPageStartsInTitle = document.activeElement === title;
  seen.placeholder = title.placeholder;
  const titleStyle = getComputedStyle(title);
  const bodyStyle = getComputedStyle(content());
  seen.measure = {
    bar: box(one('[data-slot="page-top-bar"]')), column: box(one(".page-column")),
    title: [titleStyle.fontSize, titleStyle.fontWeight, titleStyle.lineHeight].map(parseFloat), body: [bodyStyle.fontSize, bodyStyle.lineHeight].map(parseFloat),
  };
  // The title, then Enter to the body.
  type("Launch plan");
  title.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
  await until(() => content().contains(document.activeElement) || document.activeElement === content());
  seen.enterMovesToBody = document.activeElement === content();
  // Markdown shortcuts, one block each.
  for (const [marker, text] of [["# ", "One"], ["## ", "Two"], ["### ", "Three"], ["- ", "bullet"], ["1. ", "first"], ["[] ", "task"], ["> ", "quoted"]] as const) {
    caretAtEnd(); await wait(30);
    if (content().lastElementChild?.textContent) { enter(); await wait(30); }
    // A list item's Enter continues the list: a second Enter on the empty item leaves it.
    if (content().lastElementChild?.tagName === "UL" || content().lastElementChild?.tagName === "OL") { enter(); await wait(30); }
    for (const key of marker) { type(key); await wait(20); }
    type(text); await wait(30);
  }
  caretAtEnd(); enter(); await wait(30); enter(); await wait(30);
  for (const key of "--- ") { type(key); await wait(20); }
  await wait(50);
  seen.markdownBlocks = blocks();
  // `/` opens the Insert list, typing narrows it, Enter inserts.
  caretAtEnd(); await wait(30);
  type("/"); await until(() => one('[data-slot="page-insert-menu"]') !== null);
  seen.slashItems = [...document.querySelectorAll('[data-slot="page-insert-menu"] [role="option"]')].map((item) => item.textContent);
  seen.slashMenuWidth = box(one('[data-slot="page-insert-menu"]'))?.width;
  type("cod"); await wait(100);
  seen.slashFiltered = [...document.querySelectorAll('[data-slot="page-insert-menu"] [role="option"]')].map((item) => item.textContent);
  enter(); await until(() => one('[data-slot="page-insert-menu"]') === null);
  type("let x = 1;"); await wait(50);
  seen.codeBlock = content().lastElementChild?.tagName.toLowerCase();
  // Insert from the bar puts a table at the end.
  openMenu("Insert"); await until(() => menuItems().length > 0);
  seen.insertItems = menuItems().map((item) => item.textContent);
  menuItems().find((item) => item.textContent?.startsWith("Table"))?.click();
  await until(() => content().querySelector("table") !== null);
  seen.table = content().querySelector("table") ? [content().querySelectorAll("tr").length, content().querySelector("tr")!.children.length] : null;
  // Selecting text shows the toolbar; B makes it bold.
  const heading = content().querySelector("h1")!;
  const range = document.createRange();
  range.selectNodeContents(heading);
  content().focus();
  getSelection()!.removeAllRanges(); getSelection()!.addRange(range);
  document.dispatchEvent(new Event("selectionchange"));
  await until(() => one('[data-slot="page-selection-toolbar"]') !== null);
  const toolbar = one('[data-slot="page-selection-toolbar"]');
  seen.toolbar = toolbar ? [...toolbar.querySelectorAll("button")].map((button) => button.getAttribute("aria-label") ?? button.textContent) : null;
  toolbar?.querySelector<HTMLButtonElement>('button:has(.font-semibold)')?.click();
  await until(() => heading.querySelector("strong, .page-bold") !== null);
  seen.boldApplied = heading.querySelector("strong, .page-bold") !== null;
  // The handle beside a block moves it.
  const body = one(".page-body")!;
  const second = content().children[1] as HTMLElement;
  const at = second.getBoundingClientRect();
  body.dispatchEvent(new MouseEvent("mousemove", { clientX: at.left + 4, clientY: at.top + 4, bubbles: true }));
  await until(() => getComputedStyle(one(".page-drag-handle")!).opacity === "1");
  const handle = one<HTMLElement>(".page-drag-handle")!;
  seen.handleShows = getComputedStyle(handle).opacity === "1";
  seen.handleBesideBlock = handle.getBoundingClientRect().right <= at.left;
  const from = handle.getBoundingClientRect();
  handle.dispatchEvent(new PointerEvent("pointerdown", { button: 0, clientX: from.left + 4, clientY: from.top + 4, bubbles: true, cancelable: true }));
  const target = content().children[4] as HTMLElement;
  const to = target.getBoundingClientRect();
  window.dispatchEvent(new PointerEvent("pointermove", { clientX: to.left + 10, clientY: to.top + 2 }));
  seen.dropLineShows = getComputedStyle(one(".page-drop-line")!).opacity === "1";
  window.dispatchEvent(new PointerEvent("pointermove", { clientX: to.left + 10, clientY: to.bottom - 2 }));
  window.dispatchEvent(new PointerEvent("pointerup", { clientX: to.left + 10, clientY: to.bottom - 2 }));
  await wait(100);
  seen.afterDrag = blocks();
  // Saved as Markdown about 600 ms after the last change.
  await wait(900);
  seen.saved = pages.get(FIRST)!.content;
  // Changed elsewhere while there are unsaved edits: ask, and Keep both loses neither.
  content().focus(); caretAtEnd(); await wait(50); type("!"); await wait(50);
  const theirs = put(FIRST, { title: "Launch plan", emoji: null, body: "Their version" });
  emitPageChanged({ id: FIRST, revision: theirs.revision });
  await until(() => one('.page-banner[data-tone="warning"]') !== null);
  const banner = one('.page-banner[data-tone="warning"]');
  seen.conflict = banner ? [banner.querySelector(".page-banner-text")?.textContent, ...[...banner.querySelectorAll("button")].map((button) => button.textContent)] : null;
  [...(banner?.querySelectorAll("button") ?? [])].find((button) => button.textContent === "Keep both")?.click();
  await until(() => sessionTabs(ID).tabs.length === 2);
  const copy = sessionTabs(ID).tabs.find((tab) => tab.kind === "page" && tab.pageId !== FIRST);
  const copyId = copy?.kind === "page" ? copy.pageId : null;
  seen.keptBoth = { mine: copyId ? pages.get(copyId)?.content.body.slice(-30) : null, title: copyId ? pages.get(copyId)?.content.title : null, theirs: pages.get(FIRST)?.content.body };
  if (copyId) pages.delete(copyId);
  useSessionTabs.setState({ sessions: { [ID]: { tabs: sessionTabs(ID).tabs.filter((tab) => tab.id === "page:fixture"), active: "page:fixture" } } });
  await until(() => one(".page-banner") === null);
  seen.editorShowsTheirs = content().textContent;
  // The File menu.
  openMenu("File menu"); await until(() => menuItems().length > 0);
  seen.fileItems = menuItems().map((item) => item.textContent);
  menuItems().find((item) => item.textContent === "Copy as Markdown")?.click(); await wait(100);
  seen.copied = copied.at(-1);
  openMenu("File menu"); await until(() => menuItems().length > 0);
  menuItems().find((item) => item.textContent === "Download")?.click(); await wait(100);
  const download = calls.find((call) => call.command === "save_document")?.payload as { name: string; text: string; choose: boolean } | undefined;
  seen.download = download ? { name: download.name, choose: download.choose, sameText: download.text === copied.at(-1) } : null;
  // Show pages (⇧⌘E) lists every page and opens one.
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "E", code: "KeyE", metaKey: true, shiftKey: true, bubbles: true, cancelable: true }));
  await until(() => document.querySelectorAll(".page-list-row").length === 2);
  seen.pageList = [...document.querySelectorAll(".page-list-row")].map((row) => row.textContent?.replace(/Just now|\d+ min ago/, "").trim());
  document.querySelectorAll<HTMLButtonElement>(".page-list-row")[1]?.click();
  await wait(100);
  seen.openedFromList = sessionTabs(ID).tabs.map((tab) => tab.kind === "page" && tab.pageId);
  useSessionTabs.setState({ sessions: { [ID]: { ...sessionTabs(ID), active: "page:fixture" } } });
  await wait(100);
  // Delete asks first, then removes the page and its tab.
  openMenu("File menu"); await until(() => menuItems().length > 0);
  menuItems().find((item) => item.textContent === "Delete")?.click();
  await until(() => one('[role="dialog"]') !== null);
  seen.deleteAsks = one('[role="dialog"] h2')?.textContent;
  [...document.querySelectorAll<HTMLButtonElement>('[role="dialog"] button')].find((button) => button.textContent === "Delete")?.click();
  await until(() => !pages.has(FIRST));
  await wait(100);
  seen.deleted = !pages.has(FIRST);
  seen.tabsAfterDelete = sessionTabs(ID).tabs.map((tab) => tab.kind === "page" && tab.pageId);
}

function Fixture() {
  const [result, setResult] = useState("");
  const session = useSessionTabs((s) => s.sessions[ID]);
  useEffect(() => {
    if (params.has("drive")) void drive().then((seen) => setResult(JSON.stringify(seen)), (error: unknown) => setResult(JSON.stringify({ error: String(error) })));
  }, []);
  return <TooltipProvider>
    <main className="bg-background text-foreground relative flex h-screen flex-col">
      {session?.tabs.map((tab) => tab.kind === "page" && (
        <div key={tab.id} className={tab.id === session.active ? "absolute inset-0 flex flex-col" : "invisible absolute inset-0 flex flex-col"}>
          <PageTabView conversationId={ID} tab={tab} active={tab.id === session.active} />
        </div>
      ))}
    </main>
    {result && <pre id="page-result">{result}</pre>}
  </TooltipProvider>;
}
const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
