/**
 * Browser profiles over a faked shell and daemon, sample data only: the toolbar button and menu,
 * New Tab, Settings › Browser's Profiles, one profile, and delete. It all works against an in-memory
 * store. ?view=toolbar|newtab|list|listmenu|create|detail|delete|agent opens one screen; the strip at
 * the bottom goes between them.
 */
import { mockIPC } from "@/ipc/nativeMocks";
import { useEffect, useLayoutEffect, useState } from "react";
import { createPortal } from "react-dom";
import { createRoot } from "react-dom/client";

import { BrowserPage } from "@/app/settings/BrowserPage";
import { SettingsNav } from "@/app/settings/SettingsNav";
import { AppSidebar, AppStrip } from "@/app/shell/AppSidebar";
import { PageCorners } from "@/app/shell/PageCorners";
import { PageSurface } from "@/app/shell/PageSurface";
import { SessionTabBar, SessionTabViews } from "@/app/shell/SessionTabBar";
import { SidebarFoot } from "@/app/shell/SidebarFoot";
import { AssistantRuntimeProvider, type ThreadMessage, useExternalStoreRuntime } from "@/components/conversation";
import { SidebarPanel, SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import { SettingsTitlebarStrip, TerminalButton, TitlebarNav } from "@/components/shared";
import type { Conversation, Project, Request, Settings } from "@/ipc/generated";
import { useBrowsers } from "@/state/browsers";
import { type BrowserProfile, useProfilePlaces } from "@/state/browserProfiles";
import { useSessionTabs } from "@/state/sessionTabs";
import { useApp } from "@/state/store";

const params = new URLSearchParams(location.search);
const view = params.get("view") ?? "toolbar";
const platform = (params.get("os") ?? "macos") as "macos";
document.documentElement.dataset.platform = platform;
const still = document.createElement("style");
still.textContent = `#root { clip-path: none !important; } h1:focus { outline: none; }${
  params.has("still") ? " *, *::before, *::after { animation: none !important; transition: none !important; }" : ""
}`;
document.head.append(still);
localStorage.removeItem("brigadier.sessionTabs");

const now = Date.now();
const PROFILES: BrowserProfile[] = [
  { id: "work", name: "Work", hue: 6 },
  { id: "personal", name: "Personal", hue: 4 },
  { id: "northwind", name: "Northwind", hue: 1 },
  { id: "testing", name: "Testing", hue: 5 },
];

const session = (id: string, title: string, age: number): Conversation =>
  ({
    id, kind: "session", projectId: "p-1", title, pinnedAtMs: null, createdAtMs: now - age, updatedAtMs: now - age,
    lifecycle: "active", forkedFrom: null, sideOf: null, quotaWait: null, seenAtMs: now,
    setup: { type: "chat", model: { provider: "claude", model: null, effort: null, fast: false }, permission: "acceptEdits" },
  }) as unknown as Conversation;
const conversations = [
  session("s1", "Check the Northwind invoice export", 60_000),
  session("s2", "Fix checkout on staging", 3_600_000),
  session("s3", "Update the billing docs", 7_200_000),
  session("s4", "Compare hosting prices", 90_000_000),
];
const SID = view === "agent" ? "s2" : "s1";

const COUNTS: Record<string, unknown> = {
  work: { signIns: 312 },
  personal: { signIns: 88 },
  northwind: { signIns: 12 },
};

mockIPC((command, payload) => {
  const args = (payload ?? {}) as Record<string, unknown>;
  if (command === "browser_site_count") return 0;
  if (["browser_history", "browser_downloads", "browser_sites"].includes(command)) return [];
  if (command === "browser_profile_counts") return COUNTS[String(args.profile)] ?? { signIns: 0 };
  if (command === "browser_profile_delete" || command === "browser_profile_clear" || command === "browser_profiles_write") return null;
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "getConversation") {
    return {
      method: req.method,
      view: {
        conversation: conversations.find((c) => c.id === req.id) ?? conversations[0], context: null, messages: { messages: [], hasMore: false }, tasks: [], approvals: [], questions: [],
        requests: [], workerSteps: [], orchestratorSteps: [], machineSteps: [], thinking: [], compactions: [], decisions: [],
        queue: { items: [], paused: false }, run: "idle", runRequest: null, head: null, ratings: {}, streaming: null, notices: [], previews: [],
      },
    };
  }
  if (req.method === "threadView") return { method: req.method, view: { entries: [], turns: [] } };
  if (req.method === "updateSettings") return { method: req.method, settings: req.settings as Settings };
  return { method: req.method };
});

const settingsView = !["toolbar", "newtab", "agent"].includes(view);
useApp.setState((state) => ({
  catalogLoaded: true,
  info: { platform, version: "fixture", processStartMs: 0, smoke: false, firstLaunch: false, budgetTolerance: 1 } as typeof state.info,
  connection: { status: "connected", daemon: null, reason: null },
  projects: { "p-1": { id: "p-1", name: "brigadier", repos: [], createdAtMs: 0 } as unknown as Project },
  conversations: Object.fromEntries(conversations.map((c) => [c.id, c])),
  settings: { ...state.settings, browserEnabled: true },
  selection: settingsView
    ? view === "detail" || view === "delete"
      ? { type: "settings", page: "browser", detail: { view: "profile", profile: view === "delete" ? "personal" : "work" } }
      : { type: "settings", page: "browser" }
    : { type: "conversation", id: SID },
}));

const favicon = (shade: number, letter: string) =>
  `data:image/svg+xml;utf8,${encodeURIComponent(`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32"><rect width="32" height="32" rx="7" fill-opacity="${shade}"/><text x="16" y="22" font-family="Helvetica" font-size="18" font-weight="700" text-anchor="middle" fill="white">${letter}</text></svg>`)}`;
const INVOICES = "https://app.northwind.example/invoices";
const MAIL = "https://mail.example.org/inbox";
const STAGING = "https://staging.northwind.example/checkout";
const setPage = (id: string, url: string, title: string, icon: string) =>
  useBrowsers.setState(({ pages }) => ({ pages: { ...pages, [id]: { url, title, favicon: icon, loading: false, ready: true, blocked: null } } }));
if (view === "toolbar" || view === "agent") {
  setPage("b1", INVOICES, "Invoices · Northwind", favicon(0.7, "N"));
  setPage("b2", MAIL, "Inbox", favicon(0.5, "M"));
  if (view === "agent") setPage("b3", STAGING, "Checkout · Staging", favicon(0.6, "S"));
}
const browserTab = (id: string, url: string, title: string, extra = {}) => ({ kind: "browser", id, opener: "chat", url, title, ...extra });
const tabs =
  view === "newtab"
    ? [{ kind: "newTab", id: "n1", opener: "chat" }]
    : view === "agent"
      ? [browserTab("b3", STAGING, "Checkout · Staging", { agent: true, driver: { label: "Claude", taskId: null } }), browserTab("b2", MAIL, "Inbox")]
      : [browserTab("b1", INVOICES, "Invoices · Northwind"), browserTab("b2", MAIL, "Inbox")];
useSessionTabs.setState({ sessions: { [SID]: { tabs: tabs as never, active: tabs[0]?.id ?? "chat" } as never } } as never);
useProfilePlaces.setState({
  profiles: PROFILES,
  defaultId: "work",
  sessions: { s1: view === "toolbar" ? "northwind" : "work", s2: "work", s3: "northwind", s4: "personal" },
  tabs: view === "agent" ? { b3: "work", b2: "personal" } : view === "toolbar" ? { b1: "northwind", b2: "personal" } : {},
});

/** Stands in for the real web page, in its place under the toolbar. */
function FakePage({ who, title, rows }: { who: string; title: string; rows: string[][] }) {
  const [host, setHost] = useState<HTMLElement | null>(null);
  useLayoutEffect(() => {
    const timer = setInterval(() => {
      const at = document.querySelector<HTMLElement>('[role="tabpanel"]:not(.behind) [data-slot="browser-page"]');
      if (!at) return;
      clearInterval(timer);
      setHost(at);
    }, 30);
    return () => clearInterval(timer);
  }, []);
  if (!host) return null;
  return createPortal(
    <div data-slot="fake-page" className="bg-card text-foreground min-h-0 min-w-0 flex-1">
      <div className="border-divider border-b px-8 py-4 text-sm">
        <b>Northwind</b><span className="text-muted-foreground ms-4">Signed in as {who}</span>
      </div>
      <div className="px-8 py-7">
        <h1 className="mb-5 text-2xl">{title}</h1>
        <table className="w-full max-w-3xl text-sm">
          <tbody>
            {rows.map((row) => (
              <tr key={row[0]} className="border-divider border-b">
                {row.map((cell) => <td key={cell} className="py-2.5 pe-6">{cell}</td>)}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>,
    host,
  );
}

function SessionMain() {
  const runtime = useExternalStoreRuntime<ThreadMessage>({ messages: [], onNew: async () => {} });
  const conversation = conversations.find((c) => c.id === SID) as Conversation;
  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <SessionTabBar id={SID} conversation={conversation} onRename={() => {}} />
      <div className="relative min-h-0 flex-1"><SessionTabViews conversationId={SID} /></div>
      {view === "toolbar" && <FakePage who="sam@northwind.example" title="Invoices" rows={[["INV-2041", "Oct 28", "$4,120.00", "Paid"], ["INV-2040", "Oct 14", "$1,860.00", "Paid"], ["INV-2039", "Sep 30", "$2,275.00", "Overdue"], ["INV-2038", "Sep 16", "$940.00", "Paid"]]} />}
      {view === "agent" && <FakePage who="sam@acme.example" title="Checkout" rows={[["Plan", "Team, annual"], ["Seats", "12"], ["Total", "$1,440.00"], ["Status", "Ready to submit"]]} />}
    </AssistantRuntimeProvider>
  );
}

const SCREENS: [string, string][] = [
  ["toolbar", "1 Toolbar"], ["newtab", "2 New Tab"], ["list", "3 Profiles"], ["listmenu", "4 Row menu"], ["create", "5 New profile"], ["detail", "6 Detail"],
  ["delete", "7 Delete"], ["agent", "8 Agent's tab"],
];

/** Goes between the screens: plain links, each a fresh start. Not part of the app. */
function Screens() {
  return createPortal(
    <nav data-slot="fixture-screens" className="pointer-events-none fixed inset-x-0 bottom-1.5 z-50 flex justify-center">
      <span className="bg-foreground text-background pointer-events-auto flex gap-0.5 rounded-lg p-1 text-2xs">
        {SCREENS.map(([id, label]) => (
          <a key={id} href={`?view=${id}`} className={id === view ? "bg-background/25 rounded-md px-2 py-0.5" : "rounded-md px-2 py-0.5"}>{label}</a>
        ))}
      </span>
    </nav>,
    document.body,
  );
}

/** Presses what a screen shows open, once its parts are there. */
function Open({ selector }: { selector: string }) {
  useEffect(() => {
    const timer = setInterval(() => {
      const target = document.querySelector<HTMLElement>(selector);
      if (!target) return;
      clearInterval(timer);
      const press = { bubbles: true, cancelable: true, button: 0, pointerType: "mouse" } as const;
      target.dispatchEvent(new PointerEvent("pointerdown", press));
      target.dispatchEvent(new MouseEvent("mousedown", press));
    }, 120);
    return () => clearInterval(timer);
  }, [selector]);
  return null;
}

const dialogButton = (text: string) => () => [...document.querySelectorAll<HTMLElement>("button")].find((button) => button.textContent?.trim().startsWith(text)) ?? null;

/** Presses a button by its words, each in turn, once it is there. */
function PressWords({ words }: { words: string[] }) {
  useEffect(() => {
    let step = 0;
    const timer = setInterval(() => {
      const word = words[step];
      if (!word) return clearInterval(timer);
      const target = dialogButton(word)();
      if (!target) return;
      target.click();
      step += 1;
    }, 150);
    return () => clearInterval(timer);
  }, [words]);
  return null;
}

function Fixture() {
  const settings = settingsView;
  return (
    <TooltipProvider>
      <div className="bg-chrome text-foreground flex h-screen flex-col">
        <SidebarProvider keepOpen={settings} navigationKey={view} className="relative min-h-0 flex-1 flex-col">
          <PageSurface />
          <PageCorners />
          <div className="ps-surface-inset pe-surface-inset pb-surface-inset relative flex min-h-0 min-w-0 flex-1">
            <SidebarPanel strip={settings ? null : <AppStrip />} foot={settings ? null : <SidebarFoot />}>
              {settings ? <SettingsNav /> : <AppSidebar />}
            </SidebarPanel>
            <main className="body-divider relative flex h-full min-w-0 flex-1">
              <div className="relative flex h-full min-w-0 flex-1 flex-col">
                {settings ? (
                  <>
                    <SettingsTitlebarStrip><TerminalButton /></SettingsTitlebarStrip>
                    <div className="min-h-0 flex-1 overflow-clip">
                      <BrowserPage />
                    </div>
                  </>
                ) : (
                  <SessionMain />
                )}
              </div>
            </main>
          </div>
          <TitlebarNav />
          {(view === "toolbar" || view === "agent") && <Open selector='[role="tabpanel"]:not(.behind) [data-slot="browser-profile-button"]' />}
          {view === "listmenu" && <Open selector='[data-profile="northwind"] button[aria-label$="options"]' />}
          {view === "create" && <PressWords words={["New profile"]} />}
          {view === "delete" && <PressWords words={["Delete profile…"]} />}
        </SidebarProvider>
      </div>
      <Screens />
    </TooltipProvider>
  );
}

createRoot(document.getElementById("root") as HTMLElement).render(<Fixture />);
