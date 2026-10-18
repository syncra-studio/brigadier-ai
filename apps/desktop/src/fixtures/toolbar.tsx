/** Headless exercise of production toolbar, workspace pane and panel entry points. */
import { AssistantRuntimeProvider, type ThreadMessage, useExternalStoreRuntime } from "@assistant-ui/react";
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { flushSync } from "react-dom";
import { createRoot } from "react-dom/client";

import { type ComposerTarget, ComposerTargetContext } from "@/app/conversation/composerTarget";
import { ConversationToolbar } from "@/app/conversation/ConversationToolbar";
import { MentionMemory } from "@/app/conversation/Mentions";
import { NewTabTools } from "@/app/conversation/NewTabTools";
import { PinnedSummary, SummaryFloat, SummaryPane } from "@/app/conversation/PinnedSummary";
import { revealNewPreviews, useSummary } from "@/app/conversation/summaryState";
import { RightSidebar, RightSidebarContext } from "@/app/conversation/RightSidebar";
import { useSidePanel } from "@/app/conversation/SidePanel";
import { WorkspacePane, WorkspacePaneContext } from "@/app/conversation/WorkspacePane";
import { fixtureInput } from "@/fixtures/input";
import { MENU_KEYS } from "@/app/paneShortcuts";
import { SummarySection } from "@/components/assistant-ui/elements/summary-section";
import { TitlebarTips } from "@/components/titlebar-button";
import { SidebarProvider } from "@/components/ui/sidebar";
import type { Conversation, EventEnvelope, Request } from "@/ipc/generated";
import { applyBoardEvents, emptyBoard, useBoard } from "@/state/board";
import { CHAT_TAB, useSessionTabs } from "@/state/sessionTabs";
import { useApp } from "@/state/store";
import { emitWorkspaceChanged } from "@/state/workspaceCache";

const id = "toolbar-fixture";
const conversation: Conversation = { id, kind: "chat", title: "Toolbar", projectId: null,
  pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0, setup: null, lifecycle: "active",
  forkedFrom: null, sideOf: null, fallback: null, quotaWait: null };
useApp.setState({ conversations: { [id]: { ...conversation, kind: "session" } }, selection: { type: "conversation", id }, pinnedSummary: true,
  info: { platform: "macos", version: "fixture", processStartMs: 0, smoke: false, firstLaunch: false, budgetTolerance: 1 } });
useBoard.setState({ board: { ...emptyBoard(id), loaded: true, previews: { preview: {
  id: "preview", conversationId: id, name: "Web", command: "pnpm dev",
  workdir: "/fixture", workspace: "/fixture", pid: null, log: null,
  state: { type: "running" }, startedAtMs: Date.now() - 90000, endedAtMs: null, kind: "web", building: false,
} } } });
let keyboardReturns = 0;
let projectChanged = false;
mockIPC((command, args) => {
  if (command === "browser_focus" && (args as { id: string | null }).id === null) keyboardReturns++;
  if (command !== "ipc_request") return null;
  const req = (args as { request: Request }).request;
  if (req.method === "listFiles") return { method: req.method, files: ["README.md"], truncated: false };
  if (req.method === "getWorkspaceTree") return { method: req.method, tree: {
    files: req.target.type === "project" ? ["package.json", "src/app.ts"] : ["README.md"], truncated: false, ignored: [], ignoredTruncated: false } };
  if (req.method === "getSourceState") return { method: req.method, state: { branch: "main", remote: null, upstream: null, ahead: 0, staged: [], changes: [], merging: null } };
  if (req.method === "listProjectFiles") return { method: req.method, files: ["src/app.ts", "package.json"], truncated: false };
  if (req.method === "getProjectSourceState") return { method: req.method, state: { branch: "trunk", remote: null, upstream: null, ahead: 0, staged: [],
    changes: [{ path: "src/app.ts", oldPath: null, status: "modified" },
      ...(projectChanged ? [{ path: "new.ts", oldPath: null, status: "untracked" }] : [])] } };
  if (req.method === "pageList") return { method: req.method, pages: [] };
  if (req.method === "previewLog") return { method: req.method, tail: "", url: null };
  return { method: req.method };
});

const mentions = new MentionMemory();
// Only the mention memory is read by the panes.
const target = { mentions } as unknown as ComposerTarget;
const session: Conversation = { ...conversation, kind: "session", setup: { type: "session", repo: "/fixture", environment: { type: "localCheckout", branch: "fixture" }, permission: "askForApproval", planMode: false, workersSeeUncommitted: null, orchestrator: { provider: "claude", model: null, effort: null } } };
const previews = useBoard.getState().board!.previews;

function Fixture() {
  const [kind, setKind] = useState<"session" | "draft" | "chat">("session");
  const [result, setResult] = useState("");
  const [foldProbe, setFoldProbe] = useState(true);
  const { panel, agents, workspace } = useSidePanel(kind === "draft" ? null : id, kind !== "chat", false, "project");
  const runtime = useExternalStoreRuntime<ThreadMessage>({ messages: [], onNew: async () => {} });
  const [seen] = useState<Record<string, unknown>>({});
  useEffect(() => {
    if (new URLSearchParams(location.search).has("manual")) return;
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
      seen.dockClosedSlide = slide("workspace-pane"); click("Right sidebar");
      setTimeout(() => { seen.dockOpeningSlides = dockSlides()?.length; });
    });
    at(1300, () => {
      seen.files = pane(); seen.dockOpenSlide = slide("workspace-pane");
      // A column at the window's right edge below the titlebar, its views' icons in a row at its top.
      const dock = document.querySelector('[data-slot="workspace-pane"]')!.getBoundingClientRect();
      const bar = document.querySelector('header[data-slot="test-toolbar"]')!.getBoundingClientRect();
      const views = document.querySelector('[data-slot="workspace-pane"] [role="tablist"]')!.getBoundingClientRect();
      seen.atEdge = dock.top === bar.bottom && views.top === dock.top && Math.abs(dock.right - window.innerWidth) < 1 && Math.abs(dock.bottom - window.innerHeight) < 1;
      seen.views = [...document.querySelectorAll('[data-slot="workspace-pane"] [role="tab"]')].map((tab) => tab.getAttribute("aria-label"));
      click("Right sidebar");
      // Through the closing slide the contents stay; once it is over they go.
      setTimeout(() => { seen.closingContents = dockContents(); });
      setTimeout(() => { seen.closedContents = dockContents(); }, 450);
    });
    at(1800, () => {
      seen.closedFiles = pane();
      window.dispatchEvent(new KeyboardEvent("keydown", { code: "KeyG", ctrlKey: true, shiftKey: true, bubbles: true, cancelable: true }));
    });
    at(2100, () => {
      seen.source = pane();
      const before = dockSlides() ?? [];
      click("Files");
      setTimeout(() => { seen.switchSlides = dockSlides()?.filter((animation) => !before.includes(animation)).length; });
    });
    at(2200, () => {
      seen.switchedFiles = pane(); seen.filesRight = right();
      Object.defineProperty(document, "hasFocus", { configurable: true, value: () => false });
      window.dispatchEvent(new KeyboardEvent("keydown", { code: "KeyP", metaKey: true, bubbles: true, cancelable: true }));
    });
    at(2500, () => {
      seen.searchFocused = document.activeElement?.getAttribute("aria-label");
      seen.keyboardReturns = keyboardReturns; Reflect.deleteProperty(document, "hasFocus");
      window.dispatchEvent(new KeyboardEvent("keydown", { code: "KeyB", metaKey: true, altKey: true, bubbles: true, cancelable: true }));
      seen.removedKeyRight = right(); seen.menuKey = MENU_KEYS["right-sidebar"] ?? null;
    });
    at(2650, () => {
      seen.keyClosed = pane();
      // ⌘P shows the closed sidebar on Files with its search focused.
      window.dispatchEvent(new KeyboardEvent("keydown", { code: "KeyP", metaKey: true, bubbles: true, cancelable: true }));
    });
    // The right panel stays in the page while closed, and the room it takes is its full width
    // from the moment it opens rather than the sliding width, frame by frame.
    const shown = () => document.querySelector<HTMLElement>('[data-slot="sidebar-wrapper"]')?.style.getPropertyValue("--right-sidebar-shown");
    at(2800, () => {
      seen.reopened = pane();
      seen.findFileFocused = document.activeElement?.getAttribute("aria-label"); seen.closedSlide = slide("right-sidebar"); click("Thread worker");
    });
    at(3100, () => {
      seen.workerRight = right(); seen.workerTab = document.querySelector('[data-slot="right-sidebar"] [data-pane]')?.getAttribute("data-pane");
      // This 800px window can't hold the thread beside both columns: the Workers panel, opened
      // last, stays and the right sidebar closes. It still sits between the content and the sidebar.
      seen.workersNarrow = pane();
      seen.workersInside = document.querySelector('[data-slot="right-sidebar"]')!.getBoundingClientRect().right <= document.querySelector('[data-slot="workspace-pane"]')!.getBoundingClientRect().left + 1;
      seen.openShown = shown(); seen.openSlide = slide("right-sidebar");
      seen.openWidth = `${document.querySelector('[data-slot="right-sidebar"] > div > div')!.getBoundingClientRect().width}px`;
      click("Close right panel"); setTimeout(() => { seen.closingShown = shown(); });
    });
    at(3400, () => {
      seen.closedRight = right(); seen.closedShown = shown(); click("Composer worker");
      // Once the click's update is in, before any frame of the slide.
      setTimeout(() => { seen.openingShown = shown(); });
    });
    at(3700, () => {
      seen.composerRight = right();
      window.dispatchEvent(new KeyboardEvent("keydown", { code: "KeyB", metaKey: true, altKey: true, bubbles: true, cancelable: true }));
    });
    at(3850, () => {
      // Opened last, the right sidebar stays and the Workers panel closes.
      seen.sidebarWins = [pane(), right()];
      window.dispatchEvent(new KeyboardEvent("keydown", { code: "KeyB", metaKey: true, altKey: true, bubbles: true, cancelable: true }));
      click("Context worker");
    });
    at(4000, () => { seen.contextWorkerRight = right(); click("Close right panel"); });
    // The preview's Activities row; opened, its command line opens the previews.
    const chip = () => document.querySelector<HTMLButtonElement>('[data-slot="summary-card"] [data-activity] > button');
    const showPreviews = () => {
      const header = chip();
      if (header?.getAttribute("aria-expanded") === "false") flushSync(() => header.click());
      header?.parentElement?.querySelector<HTMLButtonElement>('[data-slot="task-card-transcript"] button')?.click();
    };
    const arrival = (previewId: string, conversationId = id) => {
      const events = [{ stream: `conversation:${conversationId}`, event: { type: "previewUpdated", preview: { ...previews.preview!, id: previewId, conversationId } } }] as EventEnvelope[];
      revealNewPreviews(events);
      applyBoardEvents(events);
    };
    at(4320, () => {
      // Seed the shared preference even when the floating card is currently unmounted.
      document.querySelector<HTMLButtonElement>('[data-fold-probe] > header button[aria-expanded="true"]')!.click();
      setFoldProbe(false);
      document.querySelector<HTMLButtonElement>('[data-activities] > header button[aria-expanded="true"]')?.click();
    });
    at(4350, () => {
      useSummary.setState({ floating: false });
      useApp.setState({ pinnedSummary: false });
      document.querySelector<HTMLTextAreaElement>('[aria-label="Fixture composer"]')?.focus();
      const board = useBoard.getState().board!;
      useBoard.setState({ board: { ...board, loaded: false, previews: {} } });
      arrival("loading");
      seen.loadingClosed = !useSummary.getState().floating && !useApp.getState().pinnedSummary;
      useBoard.setState({ board });
      seen.loadedClosed = !useSummary.getState().floating;
      arrival("preview");
      seen.updateClosed = !useSummary.getState().floating && !useApp.getState().pinnedSummary;
      arrival("background", "other");
      seen.backgroundClosed = !useSummary.getState().floating && !useApp.getState().pinnedSummary;
      const layout = useSummary.getState().layout;
      useSummary.setState({ layout: "beside" });
      localStorage.setItem("brigadier.pinnedSummary", "false");
      arrival("beside-preview");
      seen.besideOpened = useApp.getState().pinnedSummary;
      seen.preferenceKept = localStorage.getItem("brigadier.pinnedSummary") === "false";
      useApp.setState({ pinnedSummary: false, selection: { type: "conversation", id: "other" } });
      arrival("offscreen-preview");
      useApp.setState({ selection: { type: "conversation", id } });
      seen.returnClosed = !useApp.getState().pinnedSummary && !useSummary.getState().floating;
      useSummary.setState({ layout });
      for (const tab of ["file:README.md", "browser:fixture"]) {
        useSessionTabs.setState({ sessions: { [id]: { tabs: [], active: tab } } });
        arrival(tab);
        seen[tab.startsWith("file") ? "fileTabClosed" : "browserTabClosed"] = !useSummary.getState().floating;
      }
      // Back on the chat, a preview that arrived over another tab doesn't open the card late.
      useSessionTabs.setState({ sessions: { [id]: { tabs: [], active: CHAT_TAB } } });
      seen.chatReturnClosed = !useSummary.getState().floating;
      arrival("new-preview");
    });
    at(4500, () => {
      setFoldProbe(true);
      seen.activitiesUnfolded = !!document.querySelector('[data-activities]:not([data-folded])');
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    at(4750, () => {
      seen.activitiesFoldKept = !!document.querySelector('[data-fold-probe][data-folded]');
      seen.autoEscapeClosed = !useSummary.getState().floating;
      seen.autoEscapeFocus = document.activeElement?.getAttribute("aria-label");
      arrival("focus-preview");
    });
    at(4950, () => {
      chip()?.focus();
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    at(5200, () => {
      seen.enteredEscapeFocus = document.activeElement?.getAttribute("aria-label");
      click("Context");
    });
    at(5450, () => {
      seen.manualOpenFocus = document.activeElement?.getAttribute("aria-label");
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    at(5700, () => {
      seen.manualEscapeFocus = document.activeElement?.getAttribute("aria-label");
      document.querySelector<HTMLTextAreaElement>('[aria-label="Fixture composer"]')?.focus();
      arrival("nested-preview");
    });
    at(6500, () => { seen.autoOpened = useSummary.getState().floating && !!chip(); seen.composerKeptFocus = document.activeElement?.getAttribute("aria-label") === "Fixture composer"; showPreviews(); });
    at(6600, () => {
      seen.nestedOpened = !!document.querySelector('[role="dialog"][aria-label="Previews"][data-state="open"]') && !!chip();
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    at(6700, () => { seen.previewSection = chip()?.closest("[data-activities]")?.querySelector("header")?.textContent === "Activities"; seen.nestedDismissed = !document.querySelector('[role="dialog"][aria-label="Previews"][data-state="open"]') && !!chip(); seen.previewRight = right(); click("Right sidebar"); useBoard.setState({ board: { ...useBoard.getState().board!, previews: {} } }); });
    at(6850, () => { seen.previewCleared = !chip() && !document.querySelector("[data-activities]"); useBoard.setState({ board: { ...useBoard.getState().board!, previews } }); });
    at(7000, () => { seen.previewReturned = !!chip(); setKind("draft"); });
    const buttons = (scope: string) => [...document.querySelectorAll(`${scope} button`)].map((button) => button.textContent?.trim());
    at(7200, () => { seen.draftControls = controls(); seen.draftRight = right();
      // ⇧⌘E shows Files, whether the sidebar is open or not.
      window.dispatchEvent(new KeyboardEvent("keydown", { code: "KeyE", metaKey: true, shiftKey: true, bubbles: true, cancelable: true }));
    });
    at(7600, () => {
      seen.draftSourceCount = document.querySelector('[data-slot="workspace-pane"] [data-slot="source-count"]')?.textContent ?? null;
      seen.draftFiles = pane(); seen.draftTree = document.querySelector('[data-slot="workspace-pane"] [role="tree"]')?.textContent;
      document.querySelector<HTMLButtonElement>('[data-slot="workspace-pane"] button[title="package.json"]')?.click();
    });
    at(7900, () => { seen.draftComposer = runtime.thread.composer.getState().text; seen.draftMention = [...mentions.known()]; click("Source"); });
    at(8200, () => {
      seen.draftSource = buttons('[data-slot="source-panel"]'); seen.draftCommitBox = !!document.querySelector('[aria-label="Commit message"]');
      seen.draftNote = !!document.querySelector('[data-slot="own-copy-note"]');
      click("Right sidebar");
    });
    at(8400, () => { seen.closedSource = pane(); click("Right sidebar"); });
    at(8900, () => {
      seen.worktreeSource = buttons('[data-slot="source-panel"]'); seen.worktreeCommitBox = !!document.querySelector('[aria-label="Commit message"]');
      seen.worktreeNote = document.querySelector('[data-slot="own-copy-note"]')?.textContent;
      // ⇧⌘F shows Search with its field taking the keyboard; a draft making its own copy has no Replace.
      window.dispatchEvent(new KeyboardEvent("keydown", { code: "KeyF", metaKey: true, shiftKey: true, bubbles: true, cancelable: true }));
      useApp.setState({ connection: { status: "connected", daemon: null, reason: null } });
    });
    at(8850, () => {
      projectChanged = true;
      emitWorkspaceChanged({ type: "project", projectId: "project" });
    });
    at(9000, () => {
      seen.draftSearch = pane();
      seen.sourceCountInSearch = document.querySelector('[data-slot="workspace-pane"] [data-slot="source-count"]')?.textContent;
      seen.selectedTabs = [...document.querySelectorAll('[data-slot="workspace-pane"] [role="tab"]')].map((tab) => [tab.getAttribute("aria-label"), tab.getAttribute("aria-selected"), getComputedStyle(tab).backgroundColor]);
      seen.searchKeyFocus = document.activeElement?.getAttribute("aria-label");
      seen.draftSearchReplace = !!document.querySelector('[data-slot="workspace-pane"] input[aria-label="Replace"]');
      setKind("chat");
    });
    at(9100, () => { if (document.querySelector('button[aria-label="Context"]')?.getAttribute("aria-pressed") !== "true") click("Context"); });
    at(9300, () => { seen.chatControls = controls(); seen.chatText = document.querySelector('[data-slot="summary-card"]')?.textContent; seen.chatPane = pane(); seen.savedPane = JSON.parse(localStorage.getItem("brigadier.workspacePane")!); click("Context"); });
    at(9700, () => { seen.contextClosed = document.querySelector('button[aria-label="Context"]')?.getAttribute("aria-pressed"); click("Context"); });
    at(10100, () => { seen.contextOpened = document.querySelector('button[aria-label="Context"]')?.getAttribute("aria-pressed") ?? "false"; const board = useBoard.getState().board!; useBoard.setState({ board: { ...board, context: { usedTokens: 1200, windowTokens: 10000 } } }); });
    at(10500, () => { seen.chatUsage = document.querySelector('[data-slot="summary-card"]')?.textContent; setResult(JSON.stringify(seen)); });
    return () => timers.forEach(clearTimeout);
  }, [seen, runtime]);
  return <AssistantRuntimeProvider runtime={runtime}><ComposerTargetContext.Provider value={target}>
    <WorkspacePaneContext.Provider value={workspace}><RightSidebarContext.Provider value={panel}>
      <SummaryFloat>
      {/* As ConversationView lays it out: the top bar across the window, the panels below it. */}
      <div className="bg-background text-foreground flex h-screen min-w-0 flex-1 flex-col">
        <header data-slot="test-toolbar" className="h-titlebar flex shrink-0 items-center justify-end">
          <TitlebarTips><ConversationToolbar kind={kind === "chat" ? "chat" : "session"} /></TitlebarTips>
        </header>
        <div className="flex min-h-0 min-w-0 flex-1">
        <main className="flex min-w-0 flex-1 flex-col">
            <div className="flex min-h-0 flex-1">
              <SummaryPane summary>
                {kind !== "draft" && <PinnedSummary conversation={kind === "session" ? session : conversation} />}
                <textarea aria-label="Fixture composer" />
                <NewTabTools conversationId={id} tabId="new" />
                <button aria-label="Thread worker" onClick={() => agents.setPanel("worker")}>Thread worker</button>
                <button aria-label="Composer worker" onClick={() => agents.setPanel("worker")}>Composer worker</button>
                <button aria-label="Context worker" onClick={() => agents.setPanel(null)}>Context worker</button>
              </SummaryPane>
            </div>
        </main>
        {kind !== "draft" && <RightSidebar conversationId={id} />}
        <WorkspacePane />
        </div>
      </div>
      </SummaryFloat>
      {foldProbe && <div hidden><SummarySection data-fold-probe foldKey="activities" title="Activities" /></div>}
      {result && <pre id="toolbar-result">{result}</pre>}
    </RightSidebarContext.Provider></WorkspacePaneContext.Provider>
  </ComposerTargetContext.Provider></AssistantRuntimeProvider>;
}
if (new URLSearchParams(location.search).has("live")) await fixtureInput({ type: "viewport", width: 800, height: 600 });
createRoot(document.getElementById("root")!).render(<SidebarProvider defaultOpen={false}><Fixture /></SidebarProvider>);
