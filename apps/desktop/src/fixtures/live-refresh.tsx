/**
 * How long a change on disk takes to show in the right sidebar: the production toolbar and
 * sidebar over a real daemon, its requests and its `workspaceChanged` frames passed through
 * by `scripts/measure-live-refresh.mjs`, which writes the files and times the frames.
 */
import { AssistantRuntimeProvider, type ThreadMessage, useExternalStoreRuntime } from "@assistant-ui/react";
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { type ComposerTarget, ComposerTargetContext } from "@/app/conversation/composerTarget";
import { ConversationToolbar } from "@/app/conversation/ConversationToolbar";
import { MentionMemory } from "@/app/conversation/Mentions";
import { RightSidebarContext } from "@/app/conversation/RightSidebar";
import { useSidePanel } from "@/app/conversation/SidePanel";
import { WorkspacePane, WorkspacePaneContext } from "@/app/conversation/WorkspacePane";
import { TitlebarTips } from "@/components/titlebar-button";
import { SidebarProvider } from "@/components/ui/sidebar";
import type { Conversation, Request } from "@/ipc/generated";
import { emptyBoard, useBoard } from "@/state/board";
import { useApp } from "@/state/store";
import { emitWorkspaceChanged } from "@/state/workspaceCache";
import { emitSearchUpdate } from "@/state/workspaceSearch";

declare global {
  interface Window {
    /** The probe's way to the daemon: a request's answer, or its error. */
    daemon: (request: Request) => Promise<{ value?: unknown; error?: { code: string; message: string } }>;
    /** What the daemon says on its own, passed in by the probe. */
    heard: (frame: { type: string; id?: number; update?: unknown }) => void;
  }
}

const id = "live-refresh-fixture";
const conversation: Conversation = { id, kind: "session", title: "Live refresh", projectId: null,
  pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0, setup: null, lifecycle: "active",
  forkedFrom: null, sideOf: null, fallback: null, quotaWait: null };
useApp.setState({ conversations: { [id]: conversation }, selection: { type: "conversation", id },
  connection: { status: "connected", daemon: null, reason: null },
  info: { platform: "macos", version: "fixture", processStartMs: 0, smoke: false, firstLaunch: false, budgetTolerance: 1 } });
useBoard.setState({ board: { ...emptyBoard(id), loaded: true } });

const session = { type: "conversation", conversationId: id } as const;
window.heard = (frame) => {
  if (frame.type === "workspaceChanged") emitWorkspaceChanged(session);
  if (frame.type === "search" && frame.id !== undefined) emitSearchUpdate(frame.id, frame.update as never);
};
mockIPC(async (command, args) => {
  if (command !== "ipc_request") return null;
  const answer = await window.daemon((args as { request: Request }).request);
  if (answer.error) throw answer.error;
  return answer.value;
});

const target = { mentions: new MentionMemory() } as unknown as ComposerTarget;

function Fixture() {
  const { panel, workspace } = useSidePanel(id, true, false, null);
  const runtime = useExternalStoreRuntime<ThreadMessage>({ messages: [], onNew: async () => {} });
  return <AssistantRuntimeProvider runtime={runtime}><ComposerTargetContext.Provider value={target}>
    <WorkspacePaneContext.Provider value={workspace}><RightSidebarContext.Provider value={panel}>
      <div className="bg-background text-foreground flex h-screen min-w-0 flex-1">
        <main className="flex min-w-0 flex-1 flex-col">
          <header data-slot="test-toolbar" className="h-titlebar flex shrink-0 items-center justify-end">
            <TitlebarTips><ConversationToolbar kind="session" /></TitlebarTips>
          </header>
          <div className="flex-1" />
        </main>
        <WorkspacePane />
      </div>
    </RightSidebarContext.Provider></WorkspacePaneContext.Provider>
  </ComposerTargetContext.Provider></AssistantRuntimeProvider>;
}

createRoot(document.getElementById("root")!).render(<SidebarProvider defaultOpen={false}><Fixture /></SidebarProvider>);
