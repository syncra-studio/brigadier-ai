/** App arrows and keyboard routes over real Settings pages, without a native shell. */
import { emit } from "@tauri-apps/api/event";
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { useNavigationShortcuts } from "@/app/navigationShortcuts";
import { usePaneShortcuts } from "@/app/paneShortcuts";
import { SettingsNav } from "@/app/settings/SettingsNav";
import { SettingsView } from "@/app/settings/SettingsView";
import { SidebarPanel, SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import { TitlebarNav } from "@/components/shared";
import { fixtureClick, fixtureInput } from "@/fixtures/input";
import { accounts, overviews, usageView } from "@/fixtures/providersData";
import type { PluginsView } from "@/app/settings/plugins/types";
import type { Request } from "@/ipc/generated";
import { openSettings } from "@/state/actions";
import { openAgentPage, useAgentPageAsk } from "@/state/agentPage";
import { useHistory } from "@/state/history";
import { useApp } from "@/state/store";

const plugins: PluginsView = {
  projects: [], elsewhere: [],
  items: [{
    key: "server:example", kind: "server", name: "Example server", description: "Fixture server", detail: "example-mcp",
    url: null, icon: null, on: true, scope: { type: "allProjects" }, problem: null, locked: null, removal: "everything", canShare: false, skillCount: 0, files: [],
  }],
};
const native: string[] = [];
let signIns = 0;
mockIPC((command, payload) => {
  if (command === "browser_focus") native.push("focus app");
  if (command === "browser_go") native.push("browser go");
  if (["browser_history", "browser_downloads", "browser_sites"].includes(command)) return [];
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "getAccounts": return { method: req.method, accounts: { accounts, warnings: [] } };
    case "getUsage": return { method: req.method, usage: usageView() };
    case "getPlugins": return { method: req.method, plugins };
    case "connectAgent": signIns++; break;
    default: break;
  }
  return { method: req.method };
}, { shouldMockEvents: true });
useApp.setState((state) => ({
  info: { ...state.info, platform: "macos" } as typeof state.info,
  selection: { type: "draft", kind: "chat" },
  accounts: { accounts, warnings: [] },
  providers: { ...state.providers, view: { providers: overviews, sessions: [], fixtures: [] } },
  settings: { ...state.settings, browserEnabled: true },
}));
const wait = (ms = 60) => new Promise((resolve) => setTimeout(resolve, ms));
const text = (element: Element | null) => element?.textContent?.trim() ?? "";
const title = () => text(document.querySelector("main h1"));
const snapshot = () => ({ title: title(), selection: useApp.getState().selection, index: useHistory.getState().index });
const one = <T extends HTMLElement = HTMLElement>(selector: string) => {
  const element = document.querySelector<T>(selector);
  if (!element) throw new Error(`Missing ${selector}; title ${title()}`);
  return element;
};
const button = (label: string, root = document as ParentNode) => {
  const found = [...root.querySelectorAll<HTMLButtonElement>("button")].find((node) => text(node) === label || node.getAttribute("aria-label") === label);
  if (!found) throw new Error(`Missing button ${label}; title ${title()}`);
  return found;
};
const key = (code: string, mods: KeyboardEventInit = {}) => {
  const event = new KeyboardEvent("keydown", { code, key: code === "Escape" ? "Escape" : code, bubbles: true, cancelable: true, ...mods });
  (document.activeElement ?? document.body).dispatchEvent(event);
  return event.defaultPrevented;
};
const arrows = () => one('[data-slot="titlebar-nav"]');
const arrow = (name: string) => button(name, arrows());
const back = async () => { await fixtureClick(arrow("Back")); await wait(); };
const forward = async () => { await fixtureClick(arrow("Forward")); await wait(); };
const side = (type: string) => {
  const event = new MouseEvent(type, { button: 3, bubbles: true, cancelable: true });
  window.dispatchEvent(event);
  return event.defaultPrevented;
};
const untilTitle = async (expected: string) => {
  for (let tries = 0; tries < 100 && title() !== expected; tries++) await wait();
  if (title() !== expected) throw new Error(`Expected ${expected}; got ${title()}`);
  await wait();
};

async function drive() {
  const seen: Record<string, unknown> = {};
  openSettings("general");
  await untilTitle("General");
  await fixtureClick(button("Providers", one('[data-slot="sidebar-panel"]')));
  await untilTitle("Providers");
  await fixtureClick(one('[data-slot="agent-row"][data-setting="Claude"]'));
  await wait();
  seen.detail = snapshot();
  await back();
  seen.back = snapshot();
  await forward();
  seen.forward = snapshot();
  // Focus an input which disappears when the agent's page is left by keyboard.
  one('[data-slot="settings-page"] button').focus();
  key("BracketLeft", { metaKey: true });
  await wait();
  seen.focusAfterBack = { ...snapshot(), heading: document.activeElement === document.querySelector("main h1") };
  key("BracketRight", { metaKey: true });
  await wait();
  seen.keyForward = snapshot();

  // Settings search opens detail once. Back while reveal is pending must stay on the list.
  openSettings("providers");
  await untilTitle("Providers");
  const search = one<HTMLInputElement>('input[aria-label="Search settings"]');
  search.focus();
  await fixtureInput({ type: "type", text: "Accounts" });
  await wait();
  key("ArrowDown");
  await wait();
  key("Enter");
  await wait();
  seen.search = { ...snapshot(), focused: document.activeElement === search };
  key("BracketLeft", { metaKey: true });
  await wait(250);
  seen.searchBack = { ...snapshot(), focused: document.activeElement === search };
  // First Escape clears search; the second retains the existing close-all-Settings behavior.
  key("Escape");
  await wait();
  seen.escapeSearch = { value: search.value, selection: useApp.getState().selection };
  key("Escape");
  await wait();
  seen.escapeClose = useApp.getState().selection;

  // Browser subpages and plugin detail render from the restored app destination.
  openSettings("browser");
  await untilTitle("Browser");
  const browser: unknown[] = [];
  for (const [label, heading] of [["Browsing history", "Browsing history"], ["Download history", "Download history"], ["Site settings", "Site settings"]]) {
    const manage = button("Manage", one(`[data-setting="${label}"]`));
    manage.scrollIntoView({ block: "center" });
    await wait();
    await fixtureClick(manage);
    await untilTitle(heading!);
    browser.push(snapshot());
    await back();
    browser.push(snapshot());
    await forward();
    browser.push(snapshot());
    await fixtureClick(button("Browser", one('[data-slot="settings-page"]')));
    await untilTitle("Browser");
  }
  seen.browser = browser;
  openSettings("plugins");
  await untilTitle("Plugins");
  one('[data-setting="plugin:server:example"] button').click();
  await untilTitle("Example server");
  await back();
  seen.pluginBack = snapshot();
  await forward();
  seen.pluginForward = snapshot();

  // One-shot sign-in must not run again when Forward restores the same provider.
  openSettings("providers");
  await untilTitle("Providers");
  useApp.setState((state) => ({ providers: { ...state.providers, view: {
    ...state.providers.view!, providers: overviews.map((entry) => entry.provider === "codex" && entry.status
      ? { ...entry, status: { ...entry.status, loggedIn: false } } : entry),
  } } }));
  openAgentPage("codex", true);
  await wait();
  await back();
  await forward();
  seen.signIn = { count: signIns, pending: useAgentPageAsk.getState().ask, ...snapshot() };

  const blocked: unknown[] = [];
  for (const role of ["dialog", "alertdialog", "menu", "listbox"]) {
    const owner = document.createElement("div");
    owner.setAttribute("role", role);
    document.body.append(owner);
    const index = useHistory.getState().index;
    const prevented = key("BracketLeft", { metaKey: true });
    key("Escape");
    blocked.push({ role, prevented, stayed: useHistory.getState().index === index && title().startsWith("Codex") });
    owner.remove();
  }
  seen.blocked = blocked;
  const index = useHistory.getState().index;
  key("BracketLeft", { metaKey: true, isComposing: true });
  const prevented = new KeyboardEvent("keydown", { code: "BracketLeft", metaKey: true, bubbles: true, cancelable: true });
  prevented.preventDefault();
  window.dispatchEvent(prevented);
  key("ArrowLeft", { metaKey: true });
  seen.unclaimed = useHistory.getState().index === index;

  // The View menu also works while a native browser page owns focus. Its forwarded key
  // bubbles through the browser host, and app history returns focus to the app.
  const host = document.createElement("div");
  host.dataset.pane = "browser";
  host.dataset.active = "true";
  document.body.append(host);
  Object.defineProperty(document, "hasFocus", { configurable: true, value: () => false });
  await emit("pane-shortcut", "back");
  await wait();
  Reflect.deleteProperty(document, "hasFocus");
  host.remove();
  seen.nativeMenu = { ...snapshot(), calls: [...native] };

  useApp.setState((state) => ({ info: { ...state.info!, platform: "windows" } }));
  key("BracketRight", { ctrlKey: true });
  await wait();
  seen.ctrlForward = snapshot();
  const before = useHistory.getState().index;
  const down = side("mousedown");
  const held = useHistory.getState().index === before;
  const up = side("mouseup");
  await wait();
  seen.mouse = { down, held, up, ...snapshot() };
  // Navigating somewhere new after Back drops the forward provider detail.
  openSettings("git");
  await untilTitle("Git");
  seen.truncated = { ...snapshot(), disabled: arrow("Forward").disabled };
  return seen;
}

function Fixture() {
  useNavigationShortcuts();
  usePaneShortcuts();
  const selection = useApp((s) => s.selection);
  const [result, setResult] = useState<unknown>(null);
  useEffect(() => {
    void drive().then(setResult, (error: unknown) => setResult({ error: String(error), ...snapshot() }));
  }, []);
  return (
    <TooltipProvider>
      <SidebarProvider keepOpen={selection.type === "settings"} navigationKey={JSON.stringify(selection)} className="bg-chrome text-foreground relative h-screen">
        <SidebarPanel><SettingsNav /></SidebarPanel>
        <main className="h-full min-w-0 flex-1">
          {selection.type === "settings" && <SettingsView page={selection.page} />}
        </main>
        <TitlebarNav />
      </SidebarProvider>
      {result !== null && <pre id="navigation-result">{JSON.stringify(result)}</pre>}
    </TooltipProvider>
  );
}
createRoot(one("#root")).render(<Fixture />);
