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
  useEffect(() => {
    if (new URLSearchParams(location.search).has("manual")) return;
    const seen: Record<string, unknown> = {};
    let live = true;
    // Each step waits for what the one before it should have done, so a slow machine only runs
    // slower (#85). A wait that runs out ends the run, naming what it waited for.
    const until = (check: () => unknown, what: string, timeoutMs = 5000) => new Promise<void>((resolve, reject) => {
      const start = performance.now();
      const poll = () => {
        if (!live) return;
        if (check()) resolve();
        else if (performance.now() - start > timeoutMs) reject(new Error(`timed out waiting for ${what}`));
        else setTimeout(poll, 20);
      };
      poll();
    });
    // Once the click's update is in, before any frame of a slide it starts.
    const task = () => new Promise<void>((resolve) => setTimeout(resolve));
    const click = (label: string) => document.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)?.click();
    const key = (code: string, keys: { metaKey?: boolean; altKey?: boolean; shiftKey?: boolean; ctrlKey?: boolean }) =>
      window.dispatchEvent(new KeyboardEvent("keydown", { code, ...keys, bubbles: true, cancelable: true }));
    const escape = () => document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    const focused = () => document.activeElement?.getAttribute("aria-label");
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
    // A column's width slide is over.
    const slid = (column: string) => !(document.querySelector(`[data-slot="${column}"] > div`)?.getAnimations() ?? []).some((animation) => animation.playState === "running");
    // Width slides started on the docked pane's column, and whether it holds its contents.
    const dockSlides = () => document.querySelector('[data-slot="workspace-pane"] > div')?.getAnimations() ?? null;
    const dockContents = () => !!document.querySelector('[data-slot="workspace-pane"] [data-pane] > *');
    // The right panel stays in the page while closed, and the room it takes is its full width
    // from the moment it opens rather than the sliding width, frame by frame.
    const shown = () => document.querySelector<HTMLElement>('[data-slot="sidebar-wrapper"]')?.style.getPropertyValue("--right-sidebar-shown");
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
    const floating = () => useSummary.getState().floating;
    // Closed, its closing over (a card opened next is a new one) and the focus given back: the
    // card's focus scope gives it back a task after it goes.
    const closed = async (what: string) => {
      await until(() => !floating() && !document.querySelector('[data-slot="summary-card"]'), what);
      await task();
    };
    const previewsOpen = () => !!document.querySelector('[role="dialog"][aria-label="Previews"][data-state="open"]');
    const buttons = (scope: string) => [...document.querySelectorAll(`${scope} button`)].map((button) => button.textContent?.trim());
    const sourceCount = () => document.querySelector('[data-slot="workspace-pane"] [data-slot="source-count"]')?.textContent ?? null;
    const contextPressed = () => document.querySelector('button[aria-label="Context"]')?.getAttribute("aria-pressed");
    void (async () => {
      await until(() => controls().length === 3 && document.querySelector('[data-slot="workspace-pane"] > div'), "the toolbar");
      seen.sessionControls = controls(); seen.initialRight = right();
      seen.dockClosedSlide = slide("workspace-pane"); click("Right sidebar");
      await task();
      seen.dockOpeningSlides = dockSlides()?.length;

      await until(() => pane() === "files" && slid("workspace-pane"), "Files to open");
      seen.files = pane(); seen.dockOpenSlide = slide("workspace-pane");
      // A column at the window's right edge below the titlebar, its views' icons in a row at its top.
      const dock = document.querySelector('[data-slot="workspace-pane"]')!.getBoundingClientRect();
      const bar = document.querySelector('header[data-slot="test-toolbar"]')!.getBoundingClientRect();
      const views = document.querySelector('[data-slot="workspace-pane"] [role="tablist"]')!.getBoundingClientRect();
      seen.atEdge = dock.top === bar.bottom && views.top === dock.top && Math.abs(dock.right - window.innerWidth) < 1 && Math.abs(dock.bottom - window.innerHeight) < 1;
      seen.views = [...document.querySelectorAll('[data-slot="workspace-pane"] [role="tab"]')].map((tab) => tab.getAttribute("aria-label"));
      click("Right sidebar");
      // Through the closing slide the contents stay; once it is over they go.
      await task();
      seen.closingContents = dockContents();
      seen.closedFiles = pane();
      await until(() => slid("workspace-pane") && !dockContents(), "the closed Files to drop its contents");
      seen.closedContents = dockContents();
      key("KeyG", { ctrlKey: true, shiftKey: true });

      await until(() => pane() === "source" && slid("workspace-pane"), "⌃⇧G to show Source");
      seen.source = pane();
      const before = dockSlides() ?? [];
      click("Files");
      await task();
      seen.switchSlides = dockSlides()?.filter((animation) => !before.includes(animation)).length;

      await until(() => pane() === "files", "Files to show again");
      seen.switchedFiles = pane(); seen.filesRight = right();
      Object.defineProperty(document, "hasFocus", { configurable: true, value: () => false });
      key("KeyP", { metaKey: true });

      await until(() => focused() === "Search files", "⌘P to focus Files' search");
      seen.searchFocused = focused();
      seen.keyboardReturns = keyboardReturns; Reflect.deleteProperty(document, "hasFocus");
      key("KeyB", { metaKey: true, altKey: true });
      seen.removedKeyRight = right(); seen.menuKey = MENU_KEYS["right-sidebar"] ?? null;

      await until(() => pane() === null, "⌥⌘B to hide the right sidebar");
      seen.keyClosed = pane();
      // ⌘P shows the closed sidebar on Files with its search focused.
      key("KeyP", { metaKey: true });

      await until(() => pane() === "files" && focused() === "Search files", "⌘P to show Files again");
      seen.reopened = pane();
      seen.findFileFocused = focused(); seen.closedSlide = slide("right-sidebar"); click("Thread worker");

      await until(() => right() === "expanded" && pane() === null && slid("right-sidebar"), "the Workers panel to open");
      seen.workerRight = right(); seen.workerTab = document.querySelector('[data-slot="right-sidebar"] [data-pane]')?.getAttribute("data-pane");
      // This 800px window can't hold the thread beside both columns: the Workers panel, opened
      // last, stays and the right sidebar closes. It still sits between the content and the sidebar.
      seen.workersNarrow = pane();
      seen.workersInside = document.querySelector('[data-slot="right-sidebar"]')!.getBoundingClientRect().right <= document.querySelector('[data-slot="workspace-pane"]')!.getBoundingClientRect().left + 1;
      seen.openShown = shown(); seen.openSlide = slide("right-sidebar");
      seen.openWidth = `${document.querySelector('[data-slot="right-sidebar"] > div > div')!.getBoundingClientRect().width}px`;
      click("Close right panel");
      await task();
      seen.closingShown = shown();

      await until(() => right() === "collapsed" && slid("right-sidebar"), "the Workers panel to close");
      seen.closedRight = right(); seen.closedShown = shown(); click("Composer worker");
      await task();
      seen.openingShown = shown();

      await until(() => right() === "expanded", "the Workers panel to open from the composer");
      seen.composerRight = right();
      key("KeyB", { metaKey: true, altKey: true });

      await until(() => pane() === "files" && right() === "collapsed", "⌥⌘B to show the right sidebar");
      // Opened last, the right sidebar stays and the Workers panel closes.
      seen.sidebarWins = [pane(), right()];
      key("KeyB", { metaKey: true, altKey: true });
      click("Context worker");

      await until(() => pane() === null && right() === "expanded", "the Workers panel to open from the context");
      seen.contextWorkerRight = right(); click("Close right panel");

      await until(() => right() === "collapsed", "the Workers panel to close again");
      // Seed the shared preference even when the floating card is currently unmounted.
      document.querySelector<HTMLButtonElement>('[data-fold-probe] > header button[aria-expanded="true"]')!.click();
      setFoldProbe(false);
      document.querySelector<HTMLButtonElement>('[data-activities] > header button[aria-expanded="true"]')?.click();

      await until(() => !document.querySelector("[data-fold-probe]"), "the fold probe to go");
      useSummary.setState({ floating: false });
      useApp.setState({ pinnedSummary: false });
      document.querySelector<HTMLTextAreaElement>('[aria-label="Fixture composer"]')?.focus();
      const board = useBoard.getState().board!;
      useBoard.setState({ board: { ...board, loaded: false, previews: {} } });
      arrival("loading");
      seen.loadingClosed = !floating() && !useApp.getState().pinnedSummary;
      useBoard.setState({ board });
      seen.loadedClosed = !floating();
      arrival("preview");
      seen.updateClosed = !floating() && !useApp.getState().pinnedSummary;
      arrival("background", "other");
      seen.backgroundClosed = !floating() && !useApp.getState().pinnedSummary;
      const layout = useSummary.getState().layout;
      useSummary.setState({ layout: "beside" });
      localStorage.setItem("brigadier.pinnedSummary", "false");
      arrival("beside-preview");
      seen.besideOpened = useApp.getState().pinnedSummary;
      seen.preferenceKept = localStorage.getItem("brigadier.pinnedSummary") === "false";
      useApp.setState({ pinnedSummary: false, selection: { type: "conversation", id: "other" } });
      arrival("offscreen-preview");
      useApp.setState({ selection: { type: "conversation", id } });
      seen.returnClosed = !useApp.getState().pinnedSummary && !floating();
      useSummary.setState({ layout });
      for (const tab of ["file:README.md", "browser:fixture"]) {
        useSessionTabs.setState({ sessions: { [id]: { tabs: [], active: tab } } });
        arrival(tab);
        seen[tab.startsWith("file") ? "fileTabClosed" : "browserTabClosed"] = !floating();
      }
      // Back on the chat, a preview that arrived over another tab doesn't open the card late.
      useSessionTabs.setState({ sessions: { [id]: { tabs: [], active: CHAT_TAB } } });
      seen.chatReturnClosed = !floating();
      arrival("new-preview");

      await until(() => floating() && document.querySelector("[data-activities]"), "a new preview to open the card");
      setFoldProbe(true);
      seen.activitiesUnfolded = !!document.querySelector('[data-activities]:not([data-folded])');
      escape();

      await closed("Escape to close the card");
      await until(() => document.querySelector("[data-fold-probe]"), "the fold probe to come back");
      seen.activitiesFoldKept = !!document.querySelector('[data-fold-probe][data-folded]');
      seen.autoEscapeClosed = !floating();
      seen.autoEscapeFocus = focused();
      arrival("focus-preview");

      await until(() => floating() && chip(), "a preview to open the card again");
      chip()!.focus();
      escape();

      await closed("Escape to close the entered card");
      seen.enteredEscapeFocus = focused();
      click("Context");

      // Opened by hand the card takes the keyboard, its Escape heard from then on.
      await until(() => floating() && chip() && document.activeElement?.closest("[data-radix-popper-content-wrapper]"), "Context to open the card");
      seen.manualOpenFocus = focused();
      escape();

      await closed("Escape to close the opened card");
      seen.manualEscapeFocus = focused();
      document.querySelector<HTMLTextAreaElement>('[aria-label="Fixture composer"]')?.focus();
      arrival("nested-preview");

      await until(() => floating() && chip(), "a preview to open the card under the composer");
      seen.autoOpened = floating() && !!chip(); seen.composerKeptFocus = focused() === "Fixture composer"; showPreviews();

      await until(previewsOpen, "the previews to open");
      seen.nestedOpened = previewsOpen() && !!chip();
      escape();

      await until(() => !previewsOpen(), "Escape to close the previews");
      seen.previewSection = chip()?.closest("[data-activities]")?.querySelector("header")?.textContent === "Activities"; seen.nestedDismissed = !previewsOpen() && !!chip(); seen.previewRight = right(); click("Right sidebar"); useBoard.setState({ board: { ...useBoard.getState().board!, previews: {} } });

      await until(() => !chip(), "the previews to clear");
      seen.previewCleared = !chip() && !document.querySelector("[data-activities]"); useBoard.setState({ board: { ...useBoard.getState().board!, previews } });

      await until(chip, "the previews to return");
      seen.previewReturned = !!chip(); setKind("draft");

      await until(() => !document.querySelector('[data-slot="right-sidebar"]'), "the draft");
      seen.draftControls = controls(); seen.draftRight = right();
      // ⇧⌘E shows Files, whether the sidebar is open or not.
      key("KeyE", { metaKey: true, shiftKey: true });

      await until(() => pane() === "files" && sourceCount() !== null
        && document.querySelector('[data-slot="workspace-pane"] button[title="package.json"]'), "⇧⌘E to show the draft's Files");
      seen.draftSourceCount = sourceCount();
      seen.draftFiles = pane(); seen.draftTree = document.querySelector('[data-slot="workspace-pane"] [role="tree"]')?.textContent;
      document.querySelector<HTMLButtonElement>('[data-slot="workspace-pane"] button[title="package.json"]')!.click();

      await until(() => runtime.thread.composer.getState().text, "the file's mention in the composer");
      seen.draftComposer = runtime.thread.composer.getState().text; seen.draftMention = [...mentions.known()]; click("Source");

      await until(() => pane() === "source" && document.querySelector('[data-slot="own-copy-note"]'), "the draft's Source");
      seen.draftSource = buttons('[data-slot="source-panel"]'); seen.draftCommitBox = !!document.querySelector('[aria-label="Commit message"]');
      seen.draftNote = !!document.querySelector('[data-slot="own-copy-note"]');
      click("Right sidebar");

      await until(() => pane() === null, "the right sidebar to close on Source");
      seen.closedSource = pane(); click("Right sidebar");

      await until(() => pane() === "source" && document.querySelector('[data-slot="own-copy-note"]'), "Source to reopen");
      projectChanged = true;
      emitWorkspaceChanged({ type: "project", projectId: "project" });
      seen.worktreeSource = buttons('[data-slot="source-panel"]'); seen.worktreeCommitBox = !!document.querySelector('[aria-label="Commit message"]');
      seen.worktreeNote = document.querySelector('[data-slot="own-copy-note"]')?.textContent;
      // ⇧⌘F shows Search with its field taking the keyboard; a draft making its own copy has no Replace.
      key("KeyF", { metaKey: true, shiftKey: true });
      useApp.setState({ connection: { status: "connected", daemon: null, reason: null } });

      await until(() => pane() === "search" && focused() === "Search" && sourceCount() === "2", "⇧⌘F to show Search, and Source's count to refresh");
      seen.draftSearch = pane();
      seen.sourceCountInSearch = sourceCount();
      seen.selectedTabs = [...document.querySelectorAll('[data-slot="workspace-pane"] [role="tab"]')].map((tab) => [tab.getAttribute("aria-label"), tab.getAttribute("aria-selected"), getComputedStyle(tab).backgroundColor]);
      seen.searchKeyFocus = focused();
      seen.draftSearchReplace = !!document.querySelector('[data-slot="workspace-pane"] input[aria-label="Replace"]');
      setKind("chat");

      await until(() => controls().length === 2, "the chat's toolbar");
      if (contextPressed() !== "true") click("Context");

      await until(() => contextPressed() === "true" && document.querySelector('[data-slot="summary-card"]'), "the chat's Context card");
      seen.chatControls = controls(); seen.chatText = document.querySelector('[data-slot="summary-card"]')?.textContent; seen.chatPane = pane(); seen.savedPane = JSON.parse(localStorage.getItem("brigadier.workspacePane")!); click("Context");

      await until(() => contextPressed() !== "true", "Context to close");
      seen.contextClosed = contextPressed(); click("Context");

      await until(() => contextPressed() === "true", "Context to open");
      seen.contextOpened = contextPressed() ?? "false"; const loaded = useBoard.getState().board!; useBoard.setState({ board: { ...loaded, context: { usedTokens: 1200, windowTokens: 10000 } } });

      await until(() => /tokens used/.test(document.querySelector('[data-slot="summary-card"]')?.textContent ?? ""), "the context's usage");
      seen.chatUsage = document.querySelector('[data-slot="summary-card"]')?.textContent;
    })().catch((error: unknown) => {
      seen.error = String(error);
    }).finally(() => {
      if (live) setResult(JSON.stringify(seen));
    });
    return () => {
      live = false;
    };
  }, [runtime]);
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
