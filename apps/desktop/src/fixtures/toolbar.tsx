/** Headless exercise of production toolbar, workspace pane and panel entry points. */
import { AssistantRuntimeProvider, type ThreadMessage, useExternalStoreRuntime } from "@assistant-ui/react";
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { ConversationToolbar } from "@/app/conversation/ConversationToolbar";
import { updateDraft } from "@/app/conversation/draftSetup";
import { NewTabTools } from "@/app/conversation/NewTabTools";
import { PinnedSummary, SummaryFloat, SummaryPane } from "@/app/conversation/PinnedSummary";
import { PreviewChip } from "@/app/conversation/PreviewChip";
import { RightSidebar, RightSidebarContext } from "@/app/conversation/RightSidebar";
import { useSidePanel } from "@/app/conversation/SidePanel";
import { WorkspacePane, WorkspacePaneContext } from "@/app/conversation/WorkspacePane";
import { MENU_KEYS } from "@/app/paneShortcuts";
import { TitlebarTips } from "@/components/titlebar-button";
import { SidebarProvider } from "@/components/ui/sidebar";
import type { Conversation, Request } from "@/ipc/generated";
import { emptyBoard, useBoard } from "@/state/board";
import { showPlanDoc } from "@/state/planDoc";
import { useApp } from "@/state/store";

const id = "toolbar-fixture";
const conversation: Conversation = { id, kind: "chat", title: "Toolbar", projectId: null,
  pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0, setup: null, lifecycle: "active",
  forkedFrom: null, sideOf: null, fallback: null, quotaWait: null };
useApp.setState({ conversations: { [id]: { ...conversation, kind: "session" } }, selection: { type: "conversation", id }, pinnedSummary: true,
  info: { platform: "macos", version: "fixture", processStartMs: 0, smoke: false, firstLaunch: false, budgetTolerance: 1 } });
useBoard.setState({ board: { ...emptyBoard(id), loaded: true, previews: { preview: {
  id: "preview", conversationId: id, name: "Web", command: "pnpm dev",
  workdir: "/fixture", workspace: "/fixture", pid: null, log: null,
  state: { type: "running" }, startedAtMs: 0, endedAtMs: null,
} } } });
mockIPC((command, args) => {
  if (command !== "ipc_request") return null;
  const req = (args as { request: Request }).request;
  if (req.method === "listFiles") return { method: req.method, files: ["README.md"], truncated: false };
  if (req.method === "getSourceState") return { method: req.method, state: { branch: "main", remote: null, upstream: null, ahead: 0, staged: [], changes: [] } };
  if (req.method === "listProjectFiles") return { method: req.method, files: ["src/app.ts", "package.json"], truncated: false };
  if (req.method === "getProjectSourceState") return { method: req.method, state: { branch: "trunk", remote: null, upstream: null, ahead: 0, staged: [],
    changes: [{ path: "src/app.ts", oldPath: null, status: "modified" }] } };
  if (req.method === "previewLog") return { method: req.method, tail: "", url: null };
  return { method: req.method };
});

function Fixture() {
  const [kind, setKind] = useState<"session" | "draft" | "chat">("session");
  const [result, setResult] = useState("");
  const { panel, agents, workspace } = useSidePanel(kind === "draft" ? null : id, kind !== "chat", false, "project");
  const runtime = useExternalStoreRuntime<ThreadMessage>({ messages: [], onNew: async () => {} });
  const [seen] = useState<Record<string, unknown>>({});
  useEffect(() => {
    const timers: ReturnType<typeof setTimeout>[] = [];
    const at = (ms: number, action: () => void) => timers.push(setTimeout(action, ms));
    const click = (label: string) => document.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)?.click();
    const controls = () => [...document.querySelectorAll('header[data-slot="test-toolbar"] button[aria-label]')].map((button) => button.getAttribute("aria-label"));
    const right = () => document.querySelector('[data-slot="right-sidebar"]')?.getAttribute("data-state");
    const pane = () => document.querySelector('[data-slot="workspace-pane"] [data-pane]')?.getAttribute("data-pane") ?? null;
    at(1000, () => { seen.sessionControls = controls(); seen.initialRight = right(); click("Files"); });
    at(1300, () => {
      seen.files = pane();
      const header = document.querySelector('header[data-slot="test-toolbar"]')!.getBoundingClientRect();
      const dock = document.querySelector('[data-slot="workspace-pane"]')!.getBoundingClientRect();
      seen.belowToolbar = dock.top >= header.bottom;
      click("Files");
    });
    at(1600, () => { seen.closedFiles = pane(); click("Source"); });
    at(1900, () => { seen.source = pane(); click("Files"); });
    at(2200, () => {
      seen.switchedFiles = pane(); seen.filesRight = right();
      window.dispatchEvent(new KeyboardEvent("keydown", { code: "KeyP", metaKey: true, bubbles: true, cancelable: true }));
    });
    at(2500, () => {
      seen.searchFocused = document.activeElement?.getAttribute("aria-label");
      window.dispatchEvent(new KeyboardEvent("keydown", { code: "KeyB", metaKey: true, altKey: true, bubbles: true, cancelable: true }));
      seen.removedKeyRight = right(); seen.removedMenu = MENU_KEYS["right-sidebar"] ?? null;
      const find = [...document.querySelectorAll<HTMLButtonElement>("button")].find((button) => button.textContent?.includes("Find file"));
      find?.click();
    });
    at(2800, () => { seen.findFileFocused = document.activeElement?.getAttribute("aria-label"); click("Thread worker"); });
    at(3100, () => { seen.workerRight = right(); seen.workerTab = document.querySelector('[data-slot="right-sidebar"] [data-pane]')?.getAttribute("data-pane"); click("Close right panel"); });
    at(3400, () => { seen.closedRight = right(); click("Composer worker"); });
    at(3700, () => { seen.composerRight = right(); click("Close right panel"); click("Context worker"); });
    at(4000, () => { seen.contextWorkerRight = right(); click("Open plan"); });
    at(4300, () => { seen.planTab = document.querySelector('[data-slot="right-sidebar"] [data-pane]')?.getAttribute("data-pane"); click("Close right panel"); document.querySelector<HTMLButtonElement>('[data-slot="preview-chip"]')?.click(); });
    at(4700, () => { seen.previewPopover = !!document.querySelector('[aria-label="Previews"]'); seen.previewRight = right(); click("Files"); document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })); setKind("draft"); });
    const buttons = (scope: string) => [...document.querySelectorAll(`${scope} button`)].map((button) => button.textContent?.trim());
    at(5100, () => { seen.draftControls = controls(); seen.draftRight = right(); click("Files"); });
    at(5500, () => {
      seen.draftFiles = pane(); seen.draftTree = document.querySelector('[data-slot="workspace-pane"] [role="tree"]')?.textContent;
      document.querySelector<HTMLButtonElement>('[data-slot="workspace-pane"] button[title="package.json"]')?.click();
    });
    at(5800, () => { seen.draftComposer = runtime.thread.composer.getState().text; click("Source"); });
    at(6200, () => {
      seen.draftSource = buttons('[data-slot="source-panel"]'); seen.draftCommitBox = !!document.querySelector('[aria-label="Commit message"]');
      seen.draftNote = !!document.querySelector('[data-slot="own-copy-note"]'); updateDraft("project", { environment: "newWorktree" });
    });
    at(6500, () => {
      seen.worktreeSource = buttons('[data-slot="source-panel"]'); seen.worktreeCommitBox = !!document.querySelector('[aria-label="Commit message"]');
      seen.worktreeNote = document.querySelector('[data-slot="own-copy-note"]')?.textContent; setKind("chat");
    });
    at(6700, () => { click("Context"); });
    at(6900, () => { seen.chatControls = controls(); seen.chatText = document.querySelector('[data-slot="summary-card"]')?.textContent; seen.chatPane = pane(); seen.savedPane = JSON.parse(localStorage.getItem("brigadier.workspacePane")!); click("Context"); });
    at(7300, () => { seen.contextClosed = document.querySelector('button[aria-label="Context"]')?.getAttribute("aria-pressed"); click("Context"); });
    at(7700, () => { seen.contextOpened = document.querySelector('button[aria-label="Context"]')?.getAttribute("aria-pressed") ?? "false"; const board = useBoard.getState().board!; useBoard.setState({ board: { ...board, context: { usedTokens: 1200, windowTokens: 10000 } } }); });
    at(8100, () => { seen.chatUsage = document.querySelector('[data-slot="summary-card"]')?.textContent; setResult(JSON.stringify(seen)); });
    return () => timers.forEach(clearTimeout);
  }, [seen, runtime]);
  return <AssistantRuntimeProvider runtime={runtime}>
    <WorkspacePaneContext.Provider value={workspace}><RightSidebarContext.Provider value={panel}>
      <div className="bg-background text-foreground flex h-screen">
        <main className="flex min-w-0 flex-1 flex-col">
          <SummaryFloat>
            <header data-slot="test-toolbar" className="h-titlebar flex shrink-0 items-center justify-end">
              <TitlebarTips><ConversationToolbar kind={kind === "chat" ? "chat" : "session"} /></TitlebarTips>
            </header>
            <div className="flex min-h-0 flex-1">
              <SummaryPane summary>
                {kind === "chat" && <PinnedSummary conversation={conversation} />}
                <NewTabTools conversationId={id} tabId="new" />
                <button aria-label="Thread worker" onClick={() => agents.setPanel("worker")}>Thread worker</button>
                <button aria-label="Composer worker" onClick={() => agents.setPanel("worker")}>Composer worker</button>
                <button aria-label="Context worker" onClick={() => agents.setPanel(null)}>Context worker</button>
                <button aria-label="Open plan" onClick={() => { showPlanDoc(id, { type: "plan", id: "plan" }); panel.openTab("plan"); }}>Open plan</button>
                <PreviewChip conversationId={id} />
              </SummaryPane>
              <WorkspacePane />
            </div>
          </SummaryFloat>
        </main>
        {kind !== "draft" && <RightSidebar conversationId={id} />}
      </div>
      {result && <pre id="toolbar-result">{result}</pre>}
    </RightSidebarContext.Provider></WorkspacePaneContext.Provider>
  </AssistantRuntimeProvider>;
}
createRoot(document.getElementById("root")!).render(<SidebarProvider defaultOpen={false}><Fixture /></SidebarProvider>);
