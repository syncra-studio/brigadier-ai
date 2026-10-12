/**
 * How long the right sidebar takes to show Files or Source: the production toolbar and sidebar
 * over IPC answered after `?delay=` ms (a daemon round trip), measured frame by frame from the
 * key that opens it by `scripts/measure-pane-open.mjs`.
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
import type { Conversation, Request, SourceFile } from "@/ipc/generated";
import { emptyBoard, useBoard } from "@/state/board";
import { useApp } from "@/state/store";

const params = new URLSearchParams(location.search);
const delay = Number(params.get("delay") ?? 25);
const id = "pane-open-fixture";
const conversation: Conversation = { id, kind: "session", title: "Pane open", projectId: null,
  pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0, setup: null, lifecycle: "active",
  forkedFrom: null, sideOf: null, fallback: null, quotaWait: null };
useApp.setState({ conversations: { [id]: conversation }, selection: { type: "conversation", id },
  info: { platform: "macos", version: "fixture", processStartMs: 0, smoke: false, firstLaunch: false, budgetTolerance: 1 } });
useBoard.setState({ board: { ...emptyBoard(id), loaded: true } });

// A checkout about this repository's size: 1,500 files in nested folders, a few changed.
const files = Array.from({ length: 1500 }, (_, index) =>
  `${["apps", "crates", "docs", "scripts"][index % 4]}/part-${index % 37}/file-${index}.ts`).toSorted();
const changes: SourceFile[] = files.slice(0, 12).map((path, index) => ({ path, oldPath: null, status: index % 3 ? "modified" : "untracked" }));
const answer = (req: Request): unknown => {
  if (req.method === "listFiles") return { method: req.method, files, truncated: false };
  if (req.method === "getSourceState") return { method: req.method, state: { branch: "main", remote: null, upstream: null, ahead: 0, staged: [], changes } };
  return { method: req.method };
};
mockIPC((command, args) => {
  if (command !== "ipc_request") return null;
  const req = (args as { request: Request }).request;
  return new Promise((resolve) => setTimeout(() => resolve(answer(req)), delay));
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
