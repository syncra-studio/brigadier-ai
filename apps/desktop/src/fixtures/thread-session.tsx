/**
 * A standalone Vite entry, excluded from the app entry and production build: a recorded thread
 * session (T1's request on a dev daemon, 2026-10-08) replayed through the board reducer from its
 * stored events (`boards/thread-t1-2026-10-08.events.json`), in the app's own window layout.
 * Requests are answered here and never reach a daemon.
 *
 * Query: `density=compact`; `sidebar=0` (closed); `at=<ms>` (only the events up to that time, a
 * live moment of the session); `done=1` (the turn that waits on the user's merge answer as done,
 * its work folded).
 */
import type { Channel } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { usePaneShortcuts } from "@/app/paneShortcuts";
import { newSessionTab, sessionTabs, openReviewTab } from "@/state/sessionTabs";
import type { BrowserEvent } from "@/ipc/generated";
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { AppSidebar, AppStrip, TitlebarNav } from "@/app/AppSidebar";
import { ConversationView } from "@/app/ConversationView";
import { SidebarFoot } from "@/app/SidebarFoot";
import recorded from "@/fixtures/boards/thread-t1-2026-10-08.events.json";
import { SidebarPanel, SidebarProvider } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { Conversation, EventEnvelope, Message, Request } from "@/ipc/generated";
import { applyToBoard, emptyBoard, useBoard } from "@/state/board";
import { emptyThread, useApp } from "@/state/store";

const query = new URLSearchParams(location.search);
document.documentElement.dataset.density = query.get("density") === "compact" ? "compact" : "normal";
const id = recorded.conversationId;
const tabFixture = query.has("tabs");
const until = Number(query.get("at") ?? Number.POSITIVE_INFINITY);
const events = (recorded.events as unknown as EventEnvelope[])
  .filter((envelope) => envelope.atMs <= until)
  .map((envelope) =>
    query.get("done") === "1" && envelope.event.type === "requestUpdated" && envelope.event.request.state.type === "waiting"
      ? { ...envelope, event: { ...envelope.event, request: { ...envelope.event.request, state: { type: "done" as const }, endedAtMs: envelope.atMs } } }
      : envelope,
  );

let board = emptyBoard(id);
const messages: Message[] = [];
for (const envelope of events) {
  board = applyToBoard(board, envelope);
  // The store gives a message its place: its sequence in the conversation stream.
  if (envelope.event.type === "messageAppended") messages.push({ ...envelope.event.message, seq: envelope.streamSeq });
}
const created = events[0]?.atMs ?? 0;
const conversation = {
  id,
  kind: query.get("plain") === "1" ? "chat" : "session",
  projectId: null,
  title: "Sidebar toggle chevron",
  pinnedAtMs: null,
  createdAtMs: created,
  updatedAtMs: events.at(-1)?.atMs ?? created,
  lifecycle: query.get("archived") === "1" ? "archived" : "active",
  forkedFrom: null,
  sideOf: null,
  fallback: null,
  quotaWait: null,
  setup: {
    type: "session",
    repo: "/tmp/thread-fixture",
    environment: { type: "newWorktree", branch: "brigadier/01a11b0e/session", base: "main" },
    permission: "fullAccess",
    planMode: false,
    workersSeeUncommitted: null,
    orchestrator: { provider: "claude", model: "opus", effort: "high" },
  },
} as unknown as Conversation;

useBoard.setState({ board: { ...board, loaded: true, head: messages.at(-1)?.id ?? null } });
useApp.setState({
  ...(tabFixture ? { info: { platform: query.get("platform") ?? "macos", version: "fixture", processStartMs: 0, smoke: false, firstLaunch: false, budgetTolerance: 1 } } : {}),
  selection: { type: "conversation", id },
  conversations: { [id]: conversation },
  threads: { [id]: { ...emptyThread, items: messages } },
  pinnedSummary: false,
  connection: { status: "connected", daemon: null, reason: null },
});

const fixtureCalls: { command: string; payload: unknown }[] = [];
const browserChannels = new Map<string, Channel<BrowserEvent>>();
function finishBrowserLoad(tabId: string, url: string, favicon?: string) {
  const channel = browserChannels.get(tabId);
  channel?.onmessage({ type: "load", url, loading: false });
  channel?.onmessage({ type: "title", title: "Example Domain" });
  if (favicon) channel?.onmessage({ type: "favicon", url, dataUrl: favicon });
}
function menuShortcut(name: string) { return emit("pane-shortcut", name); }

mockIPC((command, payload) => {
  if (tabFixture && command !== "browser_place") fixtureCalls.push({ command, payload });
  if (command === "browser_open") {
    const { id: tabId, events: channel } = payload as { id: string; events: Channel<BrowserEvent> };
    browserChannels.set(tabId, channel);
    return null;
  }
  if (command === "browser_close") {
    browserChannels.delete((payload as { id: string }).id);
    return null;
  }
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "getPullRequest":
      return { method: req.method, pullRequest: null };
    case "getRunDiff":
      return { method: req.method, diff: null };
    case "listFiles":
      return { method: req.method, files: tabFixture ? ["README.md", "src/app.tsx", "src/styles.css"] : [], truncated: false };
    case "readFile":
      return { method: req.method, file: { path: req.path, text: "# Fixture file\n", size: 15, truncated: false } };
    case "openTerminal":
      return { method: req.method, terminal: { id: req.sessionId ?? "bottom", cwd: "/workspace", shell: "/bin/zsh", scrollback: "" } };
    case "listMessages":
      return { method: req.method, page: { items: [], hasMore: false } };
    default:
      return { method: req.method };
  }
}, { shouldMockEvents: true });

if (tabFixture) Object.assign(window, { sessionTabFixture: { id, fixtureCalls, finishBrowserLoad, menuShortcut } });

if (tabFixture && !sessionTabs(id).tabs.length && conversation.kind === "session") {
  const count = Number(query.get("tabs"));
  for (let index = 0; index < count; index++) {
    if (index === 0) openReviewTab(id, { type: "all" });
    else newSessionTab(id, index === count - 1 ? "newTab" : index % 2 ? "browser" : "document");
  }
}

/** The app's window layout (App.tsx) around the session, without its dialogs. */
function SessionPage() {
  usePaneShortcuts();
  return (
    <TooltipProvider>
      <div className="bg-chrome flex h-screen flex-col">
        <SidebarProvider className="relative min-h-0 flex-1 flex-col" defaultOpen={query.get("sidebar") !== "0"}>
          <div
            aria-hidden
            data-slot="page-surface"
            className="bg-background rounded-page shadow-page top-titlebar start-surface-inset end-surface-inset bottom-surface-inset pointer-events-none absolute"
          />
          <div className="relative flex min-h-0 min-w-0 flex-1 ps-surface-inset pe-surface-inset pb-surface-inset">
            <SidebarPanel strip={<AppStrip />} foot={<SidebarFoot />}>
              <AppSidebar />
            </SidebarPanel>
            <main className="body-divider relative flex h-full min-w-0 flex-1">
              <div className="relative flex h-full min-w-0 flex-1 flex-col">
                <ConversationView selection={{ type: "conversation", id }} />
              </div>
            </main>
          </div>
          <TitlebarNav />
        </SidebarProvider>
      </div>
    </TooltipProvider>
  );
}

createRoot(document.getElementById("root")!).render(<SessionPage />);
