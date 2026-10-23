/**
 * A standalone Vite entry, excluded from the app entry and production build: a Chat in the app's
 * own conversation view, IPC mocked, whose agent opens a Browser tab (#361). It checks the Chat
 * has no tab bar until then, that the tab shows in one and can be put in front and closed, and
 * that the bar goes with the last tab. What it saw goes as JSON into `<pre id="chat-tabs-result">`.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/conversation/ConversationView";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import type { Conversation, ConversationView as View, Request } from "@/ipc/generated";
import { select } from "@/state/actions";
import { CHAT_TAB, openAgentTab, sessionTabs } from "@/state/sessionTabs";
import { useApp } from "@/state/store";

const id = "tabs-chat";
const start = Date.now() - 600_000;
const conversation = {
  id, kind: "chat", projectId: null, title: "Shop check", pinnedAtMs: null, createdAtMs: start,
  updatedAtMs: start, lifecycle: "active", forkedFrom: null, sideOf: null, fallback: null, quotaWait: null,
  setup: { type: "chat", model: { provider: "claude", model: null, effort: null, fast: false }, permission: "acceptEdits" },
} as unknown as Conversation;

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "getConversation")
    return {
      method: req.method,
      view: {
        conversation, context: null, messages: { messages: [], hasMore: false }, tasks: [], approvals: [], questions: [],
        requests: [], workerSteps: [], orchestratorSteps: [], machineSteps: [], thinking: [],
        compactions: [], decisions: [], queue: { items: [], paused: false }, run: "idle",
        runRequest: null, head: null, ratings: {}, streaming: null, notices: [],
        previews: [],
      } as unknown as View,
    };
  return { method: req.method };
});
useApp.setState((state) => ({
  conversations: { [id]: conversation },
  connection: { status: "connected", daemon: null, reason: null },
  settings: { ...state.settings, browserEnabled: true },
}));

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(what: string, check: () => boolean, ms = 10_000) {
  const end = performance.now() + ms;
  while (!check()) {
    if (performance.now() > end) throw new Error(`timed out waiting for ${what}`);
    await sleep(25);
  }
}

const bar = () => document.querySelector<HTMLElement>("[data-slot=session-tabs]");
const tabNames = () => [...(bar()?.querySelectorAll<HTMLElement>("[role=tab]") ?? [])].map((tab) => ({
  selected: tab.getAttribute("aria-selected") === "true",
  id: tab.closest<HTMLElement>("[data-tab-id]")?.dataset.tabId === CHAT_TAB ? "chat" : "browser",
}));

async function drive() {
  const seen: Record<string, unknown> = {};
  try {
    await until("the Chat's composer", () => document.querySelector("[data-slot=pane-workspace] textarea, [data-slot=pane-workspace] [contenteditable=true]") !== null);
    seen.before = { bar: bar() !== null };

    // The agent's browser_open: a Browser tab behind the Chat.
    const tab = openAgentTab(id, "http://127.0.0.1:4000/", { label: "Check the shop", taskId: null, request: null, keep: false });
    await until("the tab bar", () => bar() !== null);
    seen.opened = tabNames();

    bar()!.querySelector<HTMLButtonElement>(`[data-tab-id="${CSS.escape(tab)}"] [role=tab]`)!.click();
    await until("the tab in front", () => sessionTabs(id).active === tab);
    const panel = [...document.querySelectorAll<HTMLElement>("[role=tabpanel]")].find((each) => !each.inert);
    seen.front = { tabs: tabNames(), panel: panel !== undefined && !panel.classList.contains("invisible") };

    bar()!.querySelector<HTMLButtonElement>(`[data-tab-id="${CSS.escape(tab)}"] button[aria-label^="Close"]`)!.click();
    await until("the bar gone with the tab", () => bar() === null);
    seen.closed = { tabs: sessionTabs(id).tabs.length, active: sessionTabs(id).active === CHAT_TAB };
  } catch (error) {
    seen.error = String(error);
  }
  const result = document.createElement("pre");
  result.id = "chat-tabs-result";
  result.textContent = JSON.stringify(seen);
  document.body.append(result);
}

await select({ type: "conversation", id });
createRoot(document.getElementById("root")!).render(
  <TooltipProvider>
    <SidebarProvider className="bg-background flex h-screen flex-col" defaultOpen={false}>
      <main className="relative flex min-h-0 w-full flex-1 flex-col">
        <ConversationView key={id} selection={{ type: "conversation", id }} />
      </main>
    </SidebarProvider>
  </TooltipProvider>,
);
void drive();
