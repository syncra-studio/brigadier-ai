import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { BrowserTabIcon } from "@/app/conversation/BrowserTabIcon";
import { NewTabTools } from "@/app/conversation/NewTabTools";
import { useWorkspacePane, WorkspacePaneContext } from "@/app/conversation/WorkspacePane";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { BrowserVisit, Conversation } from "@/ipc/generated";
import { useBrowsers } from "@/state/browsers";
import { newSessionTab, sessionTabs, useSessionTabs } from "@/state/sessionTabs";
import { useApp } from "@/state/store";
import { useTerminalPlaces } from "@/state/terminalPlaces";
import { useWorkspacePaneState } from "@/state/workspacePane";

const ID = "new-tab-fixture";
const GLYPH = "glyph-fixture";
const ICON = 'data:image/svg+xml,<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><rect width="16" height="16" fill="red"/></svg>';
const params = new URLSearchParams(location.search);
const calls: unknown[] = [];
let history: BrowserVisit[] = params.has("empty") ? [] : [
  { id: 3, url: "https://one.test/latest", title: "Latest site", atMs: 3, agent: false },
  { id: 2, url: "https://two.test/", title: "Second site", atMs: 2, agent: true },
  { id: 1, url: "https://one.test/old", title: "Old site", atMs: 1, agent: false },
];
const conversation: Conversation = { id: ID, kind: "session", projectId: "fixture-project", title: "New tab tools", pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0, setup: null, lifecycle: "active", forkedFrom: null, sideOf: null, fallback: null, quotaWait: null };
useApp.setState((s) => ({
  conversations: { [ID]: conversation }, selection: { type: "conversation", id: ID },
  info: { platform: "macos", version: "fixture", processStartMs: 0, smoke: false, firstLaunch: false, budgetTolerance: 1 },
  settings: { ...s.settings, browserEnabled: true },
}));
useSessionTabs.setState({ sessions: {} });
newSessionTab(ID, "newTab");
const pages = params.has("pages") ? [
  { id: "a".repeat(32), title: "Release notes", emoji: "🚀", updatedMs: Date.now() - 5 * 60_000 },
  { id: "b".repeat(32), title: "", emoji: null, updatedMs: Date.now() - 3 * 3_600_000 },
] : [];
mockIPC((command, payload) => {
  if (command === "browser_history") { calls.push(payload); return history; }
  const request = (payload as { request?: { method?: string } } | undefined)?.request;
  if (command === "ipc_request" && request?.method === "pageList") return { method: "pageList", pages };
  return null;
});
const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const until = async (check: () => boolean) => { for (let n = 0; n < 100 && !check(); n++) await wait(30); };
const one = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector);
const buttons = (root: ParentNode = document) => [...root.querySelectorAll<HTMLButtonElement>("button")];
const button = (text: string) => buttons().find((b) => b.textContent?.trim() === text || b.getAttribute("aria-label") === text);
const openMenu = (name: string) => { const b = button(name); b?.focus(); b?.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true })); };
const menuItems = () => [...document.querySelectorAll<HTMLElement>('[role="menu"][data-state="open"] [role="menuitem"]')];
const pick = (label: string) => menuItems().find((e) => e.textContent?.startsWith(label))?.click();
const box = (e: Element | null) => { const r = e?.getBoundingClientRect(); return r ? { width: r.width, height: r.height } : null; };
const page = (favicon: string | null, loading: boolean) => useBrowsers.setState({ pages: { [GLYPH]: { url: "https://one.test/", title: "Site", blocked: null, favicon, loading } } });
const currentTab = () => sessionTabs(ID).tabs[0];
const rows = () => [...document.querySelectorAll<HTMLButtonElement>('[aria-label="Recent work"] button')];
function Fixture() {
  const [result, setResult] = useState("");
  const workspace = useWorkspacePane(ID, "fixture-project", true);
  const tabId = useSessionTabs((s) => s.sessions[ID]?.tabs[0]?.id ?? "");
  useEffect(() => {
    if (!params.has("drive")) return;
    if (params.has("pages")) {
      void (async () => {
        await until(() => one('[aria-label="Recents"]') !== null);
        const seen: Record<string, unknown> = {};
        seen.sections = [...document.querySelectorAll('[data-slot="new-tab-column"] h2')].map((e) => e.textContent);
        seen.rows = rows().map((e) => e.textContent);
        rows()[1]?.click(); await wait(100);
        seen.opened = currentTab();
        // Every page tab closed: a New tab still lists the page and opens it again.
        useSessionTabs.setState({ sessions: {} }); newSessionTab(ID, "newTab"); await wait(100);
        await until(() => rows().length === 2);
        rows()[0]?.click(); await wait(100);
        seen.reopened = currentTab();
        seen.tabCount = sessionTabs(ID).tabs.length;
        setResult(JSON.stringify(seen));
      })();
      return;
    }
    void (async () => {
      await until(() => one('[data-slot="new-tab-column"]') !== null && (params.has("empty") || one('[aria-label="Suggested"]') !== null));
      await wait(100);
      const seen: Record<string, unknown> = {};
      const column = one('[data-slot="new-tab-column"]')!;
      const grid = one('[data-slot="new-tab-tools"]')!;
      const card = grid.firstElementChild!;
      seen.sections = [...column.querySelectorAll("h2")].map((e) => e.textContent);
      seen.tools = [...grid.children].map((e) => e.querySelector("button")?.textContent?.trim());
      seen.searchFields = column.querySelectorAll("input").length;
      seen.historyCalls = calls.slice();
      seen.suggested = [...column.querySelectorAll("a")].map((e) => ({ title: e.textContent, url: e.getAttribute("href") }));
      const toolsSection = grid.parentElement!.getBoundingClientRect();
      const suggestedSection = one('[aria-label="Suggested"]')?.getBoundingClientRect();
      seen.measure = { column: box(column), top: column.getBoundingClientRect().top, card: box(card), radius: parseFloat(getComputedStyle(card).borderRadius),
        rowGap: parseFloat(getComputedStyle(grid).rowGap), columnGap: parseFloat(getComputedStyle(grid).columnGap), font: parseFloat(getComputedStyle(column).fontSize),
        sectionGap: suggestedSection ? suggestedSection.top - toolsSection.bottom : null,
        headingGap: grid.getBoundingClientRect().top - grid.previousElementSibling!.getBoundingClientRect().bottom,
        tile: box(one('.new-tab-site-link')), icon: box(one('.new-tab-site-icon > svg')), overflow: document.documentElement.scrollWidth > innerWidth };
      seen.layouts = [];
      const main = one<HTMLElement>("main")!;
      for (const width of [320, 375, 414, 768]) {
        main.style.width = `${width}px`; await wait(50);
        (seen.layouts as unknown[]).push({ width, column: box(column)?.width, columns: getComputedStyle(grid).gridTemplateColumns.split(" ").length,
          fits: grid.getBoundingClientRect().right <= main.getBoundingClientRect().right && column.getBoundingClientRect().right <= main.getBoundingClientRect().right });
      }
      main.style.width = ""; await wait(50);
      if (!params.has("empty")) {
        button("Dismiss Latest site")?.click(); await wait(100);
        seen.dismissed = [...column.querySelectorAll("a")].map((e) => e.textContent);
      }
      openMenu("Terminal options"); await until(() => menuItems().length > 0);
      seen.terminalOptions = menuItems().map((e) => e.textContent);
      pick("Open bottom terminal"); await wait(100);
      seen.bottomTerminal = useTerminalPlaces.getState().places[`conv:${ID}`]?.open;
      seen.bottomKeptNewTab = currentTab()?.kind;
      openMenu("More tools…"); await until(() => menuItems().length > 0);
      seen.more = menuItems().map((e) => e.textContent);
      pick("Find file"); await wait(100);
      seen.files = { open: useWorkspacePaneState.getState().open, tab: useWorkspacePaneState.getState().tabs[ID], placeholder: currentTab()?.kind };
      button("New page")?.click(); await wait(100); seen.newPage = currentTab()?.kind;
      useSessionTabs.setState({ sessions: {} }); newSessionTab(ID, "newTab"); await wait(100);
      button("Terminal")?.click(); await wait(100); seen.terminal = currentTab()?.kind;
      useSessionTabs.setState({ sessions: {} }); newSessionTab(ID, "newTab"); await wait(100);
      button("Side chat")?.click(); await wait(100); seen.sideChat = currentTab()?.kind;
      useSessionTabs.setState({ sessions: {} }); newSessionTab(ID, "newTab"); await wait(100);
      openMenu("More tools…"); await until(() => menuItems().length > 0); pick("Review changes"); await wait(100); seen.review = currentTab()?.kind;
      if (!params.has("empty")) {
        useSessionTabs.setState({ sessions: {} }); newSessionTab(ID, "newTab"); await wait(100);
        one<HTMLAnchorElement>('.new-tab-site-link')?.click(); await wait(100); seen.siteTab = currentTab();
        history = []; useApp.setState({ selection: { type: "settings", page: "browser" } });
        await wait(50); useApp.setState({ selection: { type: "conversation", id: ID } }); await until(() => one('[aria-label="Suggested"]') === null);
        seen.clearedSuggested = one('[aria-label="Suggested"]') === null;
      }
      const glyph = () => one('[data-slot="browser-tab-icon"]')!;
      const snapshot = () => ({ phase: glyph().getAttribute("data-phase"), image: glyph().querySelector("img") !== null, globe: glyph().querySelector('.browser-tab-favicon > svg') !== null,
        visible: glyph().querySelector('.browser-tab-favicon')?.getAttribute("data-visible"), spinner: glyph().querySelector('[data-slot="favicon-spinner"]') !== null,
        animation: glyph().querySelector('[data-slot="favicon-spinner"]') ? getComputedStyle(glyph().querySelector('[data-slot="favicon-spinner"]')!).animationName : null,
        transition: getComputedStyle(glyph().querySelector('.browser-tab-favicon')!).transitionDuration,
        transform: getComputedStyle(glyph().querySelector('.browser-tab-favicon')!).transform });
      page(null, true); await wait(50); seen.spinner = snapshot();
      page(ICON, true); await until(() => glyph().getAttribute("data-phase") === "loading-favicon"); await wait(300); seen.loadingFavicon = snapshot();
      page(ICON, false); await wait(300); seen.settled = snapshot();
      page("data:image/png;base64,broken", true); await until(() => glyph().querySelector("img") === null); await wait(50); seen.failedLoading = snapshot();
      page("data:image/png;base64,broken", false); await wait(50); seen.failedSettled = snapshot();
      seen.reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;
      setResult(JSON.stringify(seen));
    })();
  }, []);
  return <TooltipProvider><WorkspacePaneContext.Provider value={workspace}>
    <main className="bg-background text-foreground flex h-screen flex-col"><NewTabTools conversationId={ID} tabId={tabId} /><div className="absolute bottom-4 left-4"><BrowserTabIcon tabId={GLYPH} /></div></main>
    {result && <pre id="new-tab-result">{result}</pre>}
  </WorkspacePaneContext.Provider></TooltipProvider>;
}
const root = document.getElementById("root");
if (root) createRoot(root).render(<Fixture />);
