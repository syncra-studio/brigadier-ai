/** Headless exercise of production toolbar, workspace pane and panel entry points. */
import { AssistantRuntimeProvider, type ThreadMessage, useExternalStoreRuntime } from "@assistant-ui/react";
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { type ComposerTarget, ComposerTargetContext } from "@/app/conversation/composerTarget";
import { ConversationToolbar } from "@/app/conversation/ConversationToolbar";
import { updateDraft } from "@/app/conversation/draftSetup";
import { MentionMemory } from "@/app/conversation/Mentions";
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

const mentions = new MentionMemory();
// Only the mention memory is read by the panes.
const target = { mentions } as unknown as ComposerTarget;
const previews = useBoard.getState().board!.previews;

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
    const pane = () => document.querySelector('[data-slot="workspace-pane"][data-state="expanded"] [data-pane]')?.getAttribute("data-pane") ?? null;
    // The right panel and the docked Files and Source pane slide like the left sidebar: the
    // column's width animates on the same spring, and not at all with reduced motion.
    const slide = (column: string) => {
      const reveal = document.querySelector(`[data-slot="${column}"] > div`);
      if (!reveal) return null;
      const probe = document.body.appendChild(document.createElement("div"));
      probe.style.transitionTimingFunction = "var(--ease-sidebar)";
      const { transitionProperty, transitionDuration, transitionTimingFunction } = getComputedStyle(reveal);
      const sidebarEase = getComputedStyle(probe).transitionTimingFunction;
      probe.remove();
      return { transitionProperty, transitionDuration, sidebarEase: transitionTimingFunction === sidebarEase, reducedMotion: reveal.className.includes("motion-reduce:transition-none") };
    };
    // Width slides started on the docked pane's column, and whether it holds its contents.
    const dockSlides = () => document.querySelector('[data-slot="workspace-pane"] > div')?.getAnimations() ?? null;
    const dockContents = () => !!document.querySelector('[data-slot="workspace-pane"] [data-pane] > *');
    at(1000, () => {
      seen.sessionControls = controls(); seen.initialRight = right();
      seen.dockClosedSlide = slide("workspace-pane"); click("Files");
      setTimeout(() => { seen.dockOpeningSlides = dockSlides()?.length; });
    });
    at(1300, () => {
      seen.files = pane(); seen.dockOpenSlide = slide("workspace-pane");
      const header = document.querySelector('header[data-slot="test-toolbar"]')!.getBoundingClientRect();
      const dock = document.querySelector('[data-slot="workspace-pane"]')!.getBoundingClientRect();
      seen.belowToolbar = dock.top >= header.bottom;
      click("Files");
      // Through the closing slide the contents stay; once it is over they go.
      setTimeout(() => { seen.closingContents = dockContents(); });
      setTimeout(() => { seen.closedContents = dockContents(); }, 450);
    });
    at(1800, () => { seen.closedFiles = pane(); click("Source"); });
    at(2100, () => {
      seen.source = pane();
      const before = dockSlides() ?? [];
      click("Files");
      setTimeout(() => { seen.switchSlides = dockSlides()?.filter((animation) => !before.includes(animation)).length; });
    });
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
    // The right panel stays in the page while closed, and the room it takes is its full width
    // from the moment it opens rather than the sliding width, frame by frame.
    const shown = () => document.querySelector<HTMLElement>('[data-slot="sidebar-wrapper"]')?.style.getPropertyValue("--right-sidebar-shown");
    at(2800, () => {
      seen.findFileFocused = document.activeElement?.getAttribute("aria-label"); seen.closedSlide = slide("right-sidebar"); click("Thread worker");
      // Once the click's update is in, before any frame of the slide.
      setTimeout(() => { seen.openingShown = shown(); });
    });
    at(3100, () => {
      seen.workerRight = right(); seen.workerTab = document.querySelector('[data-slot="right-sidebar"] [data-pane]')?.getAttribute("data-pane");
      seen.openShown = shown(); seen.openSlide = slide("right-sidebar");
      seen.openWidth = `${document.querySelector('[data-slot="right-sidebar"] > div > div')!.getBoundingClientRect().width}px`;
      click("Close right panel"); setTimeout(() => { seen.closingShown = shown(); });
    });
    at(3400, () => { seen.closedRight = right(); seen.closedShown = shown(); click("Composer worker"); });
    at(3700, () => { seen.composerRight = right(); click("Close right panel"); click("Context worker"); });
    at(4000, () => { seen.contextWorkerRight = right(); click("Open plan"); });
    at(4300, () => { seen.planTab = document.querySelector('[data-slot="right-sidebar"] [data-pane]')?.getAttribute("data-pane"); click("Close right panel"); document.querySelector<HTMLButtonElement>('[data-slot="preview-chip"]')?.click(); });
    at(4700, () => { seen.previewPopover = !!document.querySelector('[aria-label="Previews"]'); seen.previewRight = right(); click("Files"); useBoard.setState({ board: { ...useBoard.getState().board!, previews: {} } }); });
    at(4850, () => { useBoard.setState({ board: { ...useBoard.getState().board!, previews } }); });
    at(5000, () => { seen.previewReopened = !!document.querySelector('[aria-label="Previews"]'); setKind("draft"); });
    const buttons = (scope: string) => [...document.querySelectorAll(`${scope} button`)].map((button) => button.textContent?.trim());
    at(5200, () => { seen.draftControls = controls(); seen.draftRight = right(); click("Files"); });
    at(5600, () => {
      seen.draftFiles = pane(); seen.draftTree = document.querySelector('[data-slot="workspace-pane"] [role="tree"]')?.textContent;
      document.querySelector<HTMLButtonElement>('[data-slot="workspace-pane"] button[title="package.json"]')?.click();
    });
    at(5900, () => { seen.draftComposer = runtime.thread.composer.getState().text; seen.draftMention = [...mentions.known()]; click("Source"); });
    at(6200, () => {
      seen.draftSource = buttons('[data-slot="source-panel"]'); seen.draftCommitBox = !!document.querySelector('[aria-label="Commit message"]');
      seen.draftNote = !!document.querySelector('[data-slot="own-copy-note"]');
      const box = document.querySelector<HTMLTextAreaElement>('[aria-label="Commit message"]')!;
      Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!.call(box, "Unsent message");
      box.dispatchEvent(new Event("input", { bubbles: true }));
      click("Source");
    });
    at(6400, () => { seen.closedSource = pane(); click("Source"); });
    at(6600, () => {
      seen.keptMessage = document.querySelector<HTMLTextAreaElement>('[aria-label="Commit message"]')?.value;
      updateDraft("project", { environment: "newWorktree" });
    });
    at(6900, () => {
      seen.worktreeSource = buttons('[data-slot="source-panel"]'); seen.worktreeCommitBox = !!document.querySelector('[aria-label="Commit message"]');
      seen.worktreeNote = document.querySelector('[data-slot="own-copy-note"]')?.textContent; setKind("chat");
    });
    at(7100, () => { click("Context"); });
    at(7300, () => { seen.chatControls = controls(); seen.chatText = document.querySelector('[data-slot="summary-card"]')?.textContent; seen.chatPane = pane(); seen.savedPane = JSON.parse(localStorage.getItem("brigadier.workspacePane")!); click("Context"); });
    at(7700, () => { seen.contextClosed = document.querySelector('button[aria-label="Context"]')?.getAttribute("aria-pressed"); click("Context"); });
    at(8100, () => { seen.contextOpened = document.querySelector('button[aria-label="Context"]')?.getAttribute("aria-pressed") ?? "false"; const board = useBoard.getState().board!; useBoard.setState({ board: { ...board, context: { usedTokens: 1200, windowTokens: 10000 } } }); });
    at(8500, () => { seen.chatUsage = document.querySelector('[data-slot="summary-card"]')?.textContent; setResult(JSON.stringify(seen)); });
    return () => timers.forEach(clearTimeout);
  }, [seen, runtime]);
  return <AssistantRuntimeProvider runtime={runtime}><ComposerTargetContext.Provider value={target}>
    <WorkspacePaneContext.Provider value={workspace}><RightSidebarContext.Provider value={panel}>
      <div className="bg-background text-foreground flex h-screen min-w-0 flex-1">
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
  </ComposerTargetContext.Provider></AssistantRuntimeProvider>;
}
createRoot(document.getElementById("root")!).render(<SidebarProvider defaultOpen={false}><Fixture /></SidebarProvider>);
