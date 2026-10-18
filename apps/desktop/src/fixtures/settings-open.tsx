/**
 * How long Settings takes to show: the app's sidebar panel and main area as App.tsx lays them
 * out (the same lazy Settings navigation and view, behind the same Suspense), over IPC answered
 * after a daemon-like delay per method, measured frame by frame from the click by
 * `scripts/measure-settings-open.mjs`.
 *
 * The sidebar's foot has the real Settings button; `[data-open-page]` buttons open Settings at a
 * page, as the app's links into a page do (Providers from the usage menu, Import Browser Data from
 * the browser's menu). `window.ipcCalls` lists what was asked, with when. ?drive=1 opens Settings
 * and some of its pages and writes what each showed on its first frame to #settings-open-result
 * (SettingsOpenRender.test.ts reads it).
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { Suspense, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { SettingsNav, SettingsView, warmSettings } from "@/app/settings/lazy";
import { SidebarFoot } from "@/app/SidebarFoot";
import { SidebarPanel, SidebarProvider } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import { whenIdle } from "@/lib/preloadable";
import { account, OVERVIEWS, window as quotaWindow } from "@/fixtures/agents";
import type {
  AccountRef,
  AccountUsage,
  ComputerUseStatus,
  Conversation,
  ImportSource,
  ProviderKind,
  Request,
  StorageReport,
  UsageView,
} from "@/ipc/generated";
import { openSettings } from "@/state/actions";
import { type SettingsPageId, useApp } from "@/state/store";

const params = new URLSearchParams(location.search);
document.documentElement.dataset.density = "normal";
document.documentElement.dataset.platform = "macos";
// Headless Chromium in one process stops on a media permission check: Voice lists no microphones.
Object.defineProperty(navigator, "mediaDevices", {
  configurable: true,
  value: Object.assign(new EventTarget(), { enumerateDevices: () => Promise.resolve([]) }),
});

const NOW = Date.now();
const HOUR = 3_600_000;
const DAY = 24 * HOUR;
const GB = 1024 ** 3;

function conversation(id: string, title: string, fields: Partial<Conversation> = {}): Conversation {
  return {
    id, kind: "chat", title, projectId: null, pinnedAtMs: null, createdAtMs: NOW - 3 * DAY,
    updatedAtMs: NOW - 2 * DAY, setup: null, lifecycle: "archived", forkedFrom: null, sideOf: null,
    fallback: null, quotaWait: null, ...fields,
  };
}

const conversations = Object.fromEntries(
  [
    conversation("a1", "Checkout page redesign"),
    conversation("a2", "Fix flaky cart tests"),
    conversation("a3", "Add invoice export"),
    conversation("open", "Release notes", { lifecycle: "active" }),
  ].map((c) => [c.id, c]),
);

useApp.setState((state) => ({
  info: { platform: "macos", version: "fixture", processStartMs: 0, smoke: false, firstLaunch: false, budgetTolerance: 1 },
  connection: { ...state.connection, status: "connected" } as typeof state.connection,
  settings: { ...state.settings, browserEnabled: true, computerUse: true },
  providers: { ...state.providers, view: { providers: OVERVIEWS, sessions: [], fixtures: [] } },
  conversations,
  selection: { type: "conversation", id: "open" },
}));

// ----- the daemon's (and the shell's) answers --------------------------------------------------

const ACCOUNTS = [
  account("claude", undefined, "me@example.com"),
  account("claude", "acct-work", "work@example.com"),
  account("codex", undefined, "me@example.com"),
];

function usages(provider: ProviderKind): AccountUsage[] {
  return ACCOUNTS.filter((view) => view.account.provider === provider).map((view) => ({
    account: view.account as AccountRef,
    working: false,
    readAtMs: NOW - 4 * 60_000,
    quota: {
      provider,
      windows: [
        { window: quotaWindow("five_hour", "5-hour", 30, { windowMinutes: 300, resetsAtMs: NOW + 2 * HOUR }), heat: "cool", forecast: null },
        { window: quotaWindow("seven_day", "Weekly", 62, { windowMinutes: 10080, resetsAtMs: NOW + 3 * DAY }), heat: "warm", forecast: null },
      ],
      limit: null,
      heat: "warm",
      observedAtMs: NOW - 4 * 60_000,
    },
  }));
}

const USAGE: UsageView = {
  providers: [
    { provider: "claude", accounts: usages("claude"), balancing: null },
    { provider: "codex", accounts: usages("codex"), balancing: null },
  ],
  activity: [],
  // The models' plain descriptions on Providers come from here.
  models: [{
    provider: "claude", id: "opus", resolved: null, displayName: "Opus 5.5", status: "curated",
    ratingProvenance: "curated", defaultEffort: {}, registryKey: "opus", family: "opus", tier: "frontier", only: null,
    strengths: {}, areaStrengths: {}, efforts: [], contextWindow: null, knowledgeCutoff: null,
    modalities: { input: [], output: [], tools: [] }, legacy: false, excluded: false, trial: null, research: null,
  }],
  learned: [],
  projectId: null,
  registry: { revision: 1, updated: "2026-10-01", source: "bundled", models: 0, fetchedAtMs: null, checkedAtMs: null, error: null } as UsageView["registry"],
};

const COMPUTER_USE: ComputerUseStatus = {
  available: true, accessibility: "granted", screenRecording: "notGranted", current: true, setup: { state: "missing" }, sessions: [],
};

const SOURCES: ImportSource[] = ["Safari", "Google Chrome", "Arc", "Firefox"].map((name) => ({
  key: name.toLowerCase().replace(/ /g, "-"),
  name,
  family: name === "Safari" ? "safari" : name === "Firefox" ? "gecko" : "chromium",
  icon: null,
  state: "ready",
  profiles: [{ id: `${name}/Default`, name: "Default", cookies: 400, isDefault: true }],
  record: null,
}));

const STORAGE: StorageReport = {
  scanId: "scan-1",
  dataDir: "/Users/me/Library/Application Support/Brigadier",
  totalBytes: 12 * GB,
  cleanableBytes: 2 * GB,
  projects: [
    { projectId: "web", name: "storefront-web", path: "/Users/me/code/storefront-web", repoFound: true, lastUsedMs: Date.now(), conversations: 4, archived: 0, worktreesBytes: 6 * GB, brainBytes: GB, blobsBytes: GB, scratchBytes: 0 },
  ],
  shared: [
    { part: "database", bytes: GB },
    { part: "models", bytes: 2 * GB },
    { part: "logs", bytes: GB },
  ],
  items: [],
  kept: [],
};

/**
 * Each request's delay, in ms: a daemon round trip for most, longer where the work behind the
 * answer is (a usage read with its forecasts, the first permissions read asking the helper,
 * other browsers' profiles and icons, a walk of the data folder).
 */
const base = Number(params.get("delay") ?? 25);
let accessReads = 0;
function delayOf(method: string): number {
  switch (method) {
    case "getUsage":
      return 80;
    case "getUsageActivity":
      return 120;
    case "getComputerUse":
      // The first read asks the helper.
      return accessReads++ === 0 ? 400 : 40;
    case "browser_import_sources":
      return 250;
    case "scanStorage":
      return 900;
    case "login_item":
      return 10;
    default:
      return base;
  }
}

function answer(method: string, req: Request | null): unknown {
  switch (method) {
    case "login_item":
      return "off";
    case "browser_import_sources":
      return SOURCES;
    case "uninstall_app":
      return { identifier: "app.brigadier.fixture" };
  }
  switch (req?.method) {
    case "getAccounts":
      return { method: req.method, accounts: { accounts: ACCOUNTS, warnings: [] } };
    case "getUsage":
      return { method: req.method, usage: USAGE };
    case "getUsageActivity":
      return { method: req.method, activity: { sinceMs: NOW - 7 * DAY, accounts: [] } };
    case "getRankingsRefresh":
      return { method: req.method, refresh: null };
    case "previewRoutes":
      return { method: req.method, routes: [] };
    case "listMemories":
      return { method: req.method, memories: [{ id: "m1" }, { id: "m2" }] };
    case "getComputerUse":
      return { method: req.method, computerUse: COMPUTER_USE };
    case "scanStorage":
      return { method: req.method, report: STORAGE };
    default:
      return req ? { method: req.method } : null;
  }
}

const calls: { method: string; atMs: number; answered: boolean }[] = [];
(window as unknown as { ipcCalls: typeof calls }).ipcCalls = calls;
mockIPC((command, payload) => {
  const req = command === "ipc_request" ? (payload as { request: Request }).request : null;
  const method = req?.method ?? command;
  const call = { method, atMs: performance.now(), answered: false };
  calls.push(call);
  return new Promise((resolve) =>
    setTimeout(() => {
      call.answered = true;
      resolve(answer(method, req));
    }, delayOf(method)),
  );
});
/** Whether a request for `method` is still on its way. */
const waiting = (method: string) => calls.some((call) => call.method === method && !call.answered);

// ----- the layout ------------------------------------------------------------------------------

const PAGES: SettingsPageId[] = [
  "general", "voice", "configuration", "shortcuts", "providers", "routing", "git", "computerUse",
  "browser", "browserImport", "storage", "inspector", "archived",
];

// ----- ?drive=1 -------------------------------------------------------------------------------

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const frame = () => new Promise((resolve) => requestAnimationFrame(resolve));
let warm: (loads: Promise<unknown>) => void = () => {};
const warmed = new Promise<unknown>((resolve) => { warm = resolve; });
const mainText = () => document.querySelector("main")?.textContent ?? "";
const title = () => document.querySelector("main h1")?.textContent ?? null;
const navButton = (label: string) =>
  [...document.querySelectorAll<HTMLButtonElement>('[data-slot="sidebar-panel"] button')].find(
    (button) => button.textContent?.trim() === label,
  );

/** Clicks the navigation's `label` and reads the page on the first frame after. */
async function open(label: string, read: () => Record<string, unknown> = () => ({})) {
  navButton(label)?.click();
  await frame();
  return { title: title(), ...read() };
}

/**
 * Settings and its pages on the first frame after each click: the shell and the page's layout
 * at once, even while what the page reads is on its way; a page seen before shows what it read
 * then.
 */
async function drive(): Promise<string> {
  // The app's idle warm-up after startup, until its code is in: a busy machine takes longer.
  await sleep(3000);
  await warmed;
  await sleep(200);
  document.querySelector<HTMLButtonElement>('button[aria-label="Settings"]')?.click();
  await frame();
  const settings = {
    nav: document.querySelector('[data-slot="sidebar-panel"]')?.textContent?.includes("Settings") ?? false,
    // The titlebar's Back is the way back: the panel has no link of its own.
    backToApp: document.querySelector('[data-slot="sidebar-panel"]')?.textContent?.includes("Back to app") ?? true,
    title: title(),
  };
  // Settings open and idle: the Inspector's code loads.
  await sleep(1500);
  const usage = await open("Providers", () => ({ data: mainText().includes("Frontier"), reading: mainText().includes("Reading the models") }));
  const computerUse = await open("Computer use", () => ({
    status: document.querySelector('[data-slot="setup-line"]')?.textContent ?? null,
    readWaiting: waiting("getComputerUse"),
  }));
  await sleep(1000);
  const computerUseRead = document.querySelector('[data-slot="setup-line"]')?.textContent ?? null;
  const importFirst = await open("Import Browser Data", () => ({
    placeholder: Boolean(document.querySelector('[data-slot="import-listing"]')),
    loadingLine: mainText().includes("Loading…"),
    rows: document.querySelectorAll('[data-slot="import-row"]').length,
    listWaiting: waiting("browser_import_sources"),
  }));
  await sleep(1000);
  await open("General");
  const importAgain = await open("Import Browser Data", () => ({
    rows: document.querySelectorAll('[data-slot="import-row"]').length,
    listWaiting: waiting("browser_import_sources"),
  }));
  const storageFirst = await open("Storage", () => ({
    bar: Boolean(document.querySelector('[data-slot="storage-bar-pending"]')),
    scanWaiting: waiting("uninstall_app") || waiting("scanStorage"),
  }));
  await sleep(1500);
  await open("General");
  const storageAgain = await open("Storage", () => ({
    used: /used by Brigadier/.test(mainText()),
    bar: Boolean(document.querySelector('[data-slot="storage-bar"]')),
    scanWaiting: waiting("uninstall_app") || waiting("scanStorage"),
  }));
  const inspector = await open("Inspector");
  return JSON.stringify({ settings, usage, computerUse, computerUseRead, importFirst, importAgain, storageFirst, storageAgain, inspector });
}

function Result() {
  const [result, setResult] = useState<string | null>(null);
  useEffect(() => {
    void drive().then(setResult, (error: unknown) => setResult(JSON.stringify({ error: String(error) })));
  }, []);
  return result ? <pre id="settings-open-result">{result}</pre> : null;
}

function Fixture() {
  const selection = useApp((s) => s.selection);
  const settings = selection.type === "settings";
  return (
    <div className="bg-chrome text-foreground flex h-screen flex-col">
      <SidebarProvider keepOpen={settings} navigationKey={JSON.stringify(selection)} className="relative min-h-0 flex-1 flex-col">
        <div className="relative flex min-h-0 min-w-0 flex-1">
          <SidebarPanel strip={settings ? null : <div />} foot={settings ? null : <SidebarFoot />}>
            {settings ? (
              <Suspense fallback={null}>
                <SettingsNav />
              </Suspense>
            ) : (
              <div data-slot="app-sidebar" className="p-3 text-sm">Chats</div>
            )}
          </SidebarPanel>
          <main className="relative flex h-full min-w-0 flex-1 flex-col">
            {selection.type === "settings" ? (
              <div className="min-h-0 flex-1">
                <Suspense fallback={null}>
                  <SettingsView page={selection.page} />
                </Suspense>
              </div>
            ) : (
              <div data-slot="home" className="flex flex-wrap gap-1 p-3">
                {PAGES.map((page) => (
                  <button key={page} type="button" data-open-page={page} onClick={() => openSettings(page)}>
                    {page}
                  </button>
                ))}
              </div>
            )}
          </main>
        </div>
      </SidebarProvider>
    </div>
  );
}

// As the app does once it is up; `warmed` settles once the code it loads is in.
whenIdle(() => {
  warmSettings();
  warm(Promise.all([import("@/app/settings/SettingsNav"), import("@/app/settings/SettingsView"), import("@/app/settings/pages")]));
});

createRoot(document.getElementById("root")!).render(
  <TooltipProvider>
    <Fixture />
    {params.has("drive") && <Result />}
  </TooltipProvider>,
);
