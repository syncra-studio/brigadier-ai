/**
 * Headless exercise of a session's context card where it floats (a narrow window): its toolbar
 * button opens it over the thread, also while another tab is in front, which brings the chat back.
 */
import { AssistantRuntimeProvider, type ThreadMessage, useExternalStoreRuntime } from "@assistant-ui/react";
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { ConversationToolbar } from "@/app/conversation/ConversationToolbar";
import { PinnedSummary, SummaryFloat, SummaryPane } from "@/app/conversation/PinnedSummary";
import { RightSidebarContext } from "@/app/conversation/RightSidebar";
import { SessionTabBar } from "@/app/conversation/SessionTabBar";
import { useSidePanel } from "@/app/conversation/SidePanel";
import { WorkspacePane, WorkspacePaneContext } from "@/app/conversation/WorkspacePane";
import { SidebarProvider } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { Conversation, Request } from "@/ipc/generated";
import { emptyBoard, useBoard } from "@/state/board";
import { CHAT_TAB, newSessionTab, selectTab, sessionTabs, useSessionTabs } from "@/state/sessionTabs";
import { useApp } from "@/state/store";

const id = "context-card-fixture";
useSessionTabs.setState({ sessions: {} });
const conversation: Conversation = {
  id, kind: "session", projectId: null, title: "Tidy the release notes",
  pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0, setup: null, lifecycle: "active",
  forkedFrom: null, sideOf: null, fallback: null, quotaWait: null,
};
useApp.setState({
  conversations: { [id]: conversation }, selection: { type: "conversation", id }, pinnedSummary: true,
  info: { platform: "macos", version: "fixture", processStartMs: 0, smoke: false, firstLaunch: false, budgetTolerance: 1 },
  connection: { status: "connected", daemon: null, reason: null },
});
useBoard.setState({ board: { ...emptyBoard(id), loaded: true } });
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  return { method: (payload as { request: Request }).request.method };
});

const toggle = () => document.querySelector<HTMLButtonElement>('header button[aria-label="Context"]')!;
// The card where it floats: the popover, not the button nor the pinned column.
const floating = () => [...document.querySelectorAll('[aria-label="Context"]')].some((element) => element.getAttribute("role") === "dialog" && element.getAttribute("data-state") === "open");

function Fixture() {
  const runtime = useExternalStoreRuntime<ThreadMessage>({ messages: [], onNew: async () => {} });
  const { panel, workspace } = useSidePanel(id, true, false, "project");
  const [result, setResult] = useState("");
  useEffect(() => {
    const seen: Record<string, unknown> = {};
    const timers: ReturnType<typeof setTimeout>[] = [];
    const at = (ms: number, action: () => void) => timers.push(setTimeout(action, ms));
    at(600, () => {
      seen.layout = document.querySelector<HTMLElement>("[data-summary]")?.dataset.summary;
      newSessionTab(id, "newTab", "/fixture");
    });
    at(1000, () => {
      seen.coveredBefore = sessionTabs(id).active !== CHAT_TAB;
      toggle().click();
    });
    at(1500, () => {
      seen.chatInFront = sessionTabs(id).active === CHAT_TAB;
      seen.openedOverTab = floating();
      toggle().click();
    });
    at(1900, () => {
      seen.closed = !floating();
      toggle().click();
    });
    at(2300, () => {
      seen.openedOverChat = floating();
      setResult(JSON.stringify(seen));
    });
    return () => timers.forEach(clearTimeout);
  }, []);
  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <WorkspacePaneContext.Provider value={workspace}>
        <RightSidebarContext.Provider value={panel}>
          <main className="bg-background text-foreground w-narrowest-window flex h-screen min-w-0 flex-col">
            <SummaryFloat>
              <SessionTabBar conversation={conversation} onRename={() => {}}>
                <ConversationToolbar kind="session" onContext={() => selectTab(id, CHAT_TAB)} />
              </SessionTabBar>
              <div className="relative flex min-h-0 flex-1">
                <SummaryPane summary>
                  <PinnedSummary conversation={conversation} />
                </SummaryPane>
                <WorkspacePane />
              </div>
            </SummaryFloat>
            {result && <pre id="context-card-result">{result}</pre>}
          </main>
        </RightSidebarContext.Provider>
      </WorkspacePaneContext.Provider>
    </AssistantRuntimeProvider>
  );
}

createRoot(document.getElementById("root")!).render(
  <SidebarProvider defaultOpen={false}><TooltipProvider><Fixture /></TooltipProvider></SidebarProvider>,
);
