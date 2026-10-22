import { revealNewPreviews } from "@/app/conversation/summaryState";
import { subscribe } from "@/ipc/client";
import type { BridgeEvent, EventEnvelope } from "@/ipc/generated";
import { markApplied, noteFlush, setSamplingPaused } from "@/lib/perf";
import { markStartup } from "@/lib/startup";
import {
  loadCatalog,
  loadConversation,
  openConversation,
  openOrchestratorLog,
  openSettings,
  toggleSettings,
} from "@/state/actions";
import { applyActivityEvents, loadActivity } from "@/state/activity";
import { applyThreadNews, resyncThreadViews } from "@/state/cliThread";
import { applySettledTabEvents, loadSettledTabs, refreshSettledTabsSoon } from "@/state/tabStates";
import { openFolderPicker, takeFolders } from "@/state/addProject";
import { applyBoardEvents, forgetKeptBoards, sideBoardIds, useBoard } from "@/state/board";
import { applyBrainEvents, reloadSessionTasks } from "@/state/brain";
import { runAppCall } from "@/state/appCalls";
import { onComputerUse } from "@/state/computerUse";
import { applyEvents, useApp } from "@/state/store";
import { startBrowserOptions } from "@/state/links";
import { startMenuBar } from "@/state/menuBar";
import { emitPageChanged } from "@/state/pageChanges";
import { emitWorkspaceChanged } from "@/state/workspaceCache";
import { emitSearchUpdate } from "@/state/workspaceSearch";
import { startAppUpdate } from "@/state/appUpdate";
import { startPagesFlush } from "@/state/pagesFlush";
import { applyUpdateEvents, loadUpdates } from "@/state/updates";
import { applyConnectEvents, loadConnect } from "@/state/connect";
import { applyUsageEvents } from "@/state/usage";
import { onDictationDisconnected, onDictationUpdate } from "@/state/dictation";
import { openFeedback } from "@/state/feedback";
import { emitCleanProgress, openUninstall } from "@/state/storage";
import { emitTerminalOutput } from "@/state/terminalPlaces";

let queued: EventEnvelope[] = [];
let flushScheduled = false;

/** Events arrive one message at a time; everything that arrived in the same task is applied
 * in a single state update, then timed to the paint that shows it. */
function flush() {
  flushScheduled = false;
  const batch = queued;
  queued = [];
  const started = performance.now();
  applyEvents(batch);
  revealNewPreviews(batch);
  applyBoardEvents(batch);
  applyActivityEvents(batch);
  applySettledTabEvents(batch);
  applyBrainEvents(batch);
  applyUsageEvents(batch);
  applyUpdateEvents(batch);
  applyConnectEvents(batch);
  noteFlush(started, batch.map(({ event }) => event.type));
  for (const { atMs, event } of batch) {
    markApplied(atMs, event.type === "probe" ? event.burstId : null);
  }
}

/** Reloads everything the UI holds: used after (re)connecting and after missing events.
 * Threads and kept boards other than the shown ones may be stale, so they are dropped and reload
 * when opened. The shown conversations' boards, the Inspector's orchestrator log and the Brain
 * tab's session workers are read again. */
async function resync() {
  const { selection, threads } = useApp.getState();
  const openId = selection.type === "conversation" ? selection.id : null;
  // The open conversation and the side chat beside it keep their threads.
  const shown = [...(openId ? [openId] : []), ...sideBoardIds()];
  useApp.setState({
    threads: Object.fromEntries(
      shown.flatMap((id) => (threads[id] ? [[id, threads[id]] as const] : [])),
    ),
  });
  // So are the boards kept of the conversations shown before.
  forgetKeptBoards();
  // Ready and merged count commits in each tab's worktree: they come when they come.
  void loadSettledTabs().catch(() => {});
  await Promise.all([loadCatalog(), loadActivity(), loadUpdates(), loadConnect()]);
  const log = useBoard.getState().orchestrator;
  if (log) void openOrchestratorLog(log.conversationId);
  await Promise.all([...shown.map((id) => loadConversation(id, { fresh: true })), reloadSessionTasks(), resyncThreadViews(shown)]);
}

/** Whether the daemon has answered yet: startup marks the first time. */
let connectedOnce = false;

function onBridgeEvent(message: BridgeEvent) {
  switch (message.type) {
    case "event":
      queued.push(message.event);
      if (!flushScheduled) {
        flushScheduled = true;
        queueMicrotask(flush);
      }
      break;
    case "connected":
      if (!connectedOnce) {
        connectedOnce = true;
        markStartup("connected");
      }
      useApp.setState({
        connection: { status: "connected", daemon: message.daemon, reason: null },
      });
      // Folders opened with the app before it connected are added once the catalog is in.
      void resync()
        .then(takeFolders)
        .catch((error: unknown) => {
          console.error("resync after connecting failed", error);
        });
      break;
    case "disconnected":
      onDictationDisconnected();
      useApp.setState((state) => ({
        connection: {
          status: "disconnected",
          daemon: state.connection.daemon,
          reason: message.reason,
        },
      }));
      break;
    case "lagged":
      void resync().catch((error: unknown) => {
        console.error("resync after lag failed", error);
      });
      break;
    case "metrics":
      useApp.setState((state) => ({
        inspector: { ...state.inspector, metrics: message.metrics },
      }));
      break;
    case "terminal":
      emitTerminalOutput(message.output);
      break;
    case "dictation":
      onDictationUpdate(message.update);
      break;
    case "computerUse":
      onComputerUse(message.status);
      break;
    case "appCall":
      void runAppCall(message.call);
      break;
    case "pageChanged":
      emitPageChanged({ id: message.id, revision: message.revision });
      break;
    case "storageClean":
      emitCleanProgress(message.progress);
      break;
    case "threadNews":
      applyThreadNews(message.news);
      break;
    case "workspaceChanged":
      emitWorkspaceChanged(message.target);
      refreshSettledTabsSoon();
      break;
    case "search":
      emitSearchUpdate(message.id, message.update);
      break;
    case "windowVisibility":
      setSamplingPaused(!message.visible);
      useApp.setState({ windowVisible: message.visible });
      break;
    case "openConversation":
      openConversation(message.conversationId);
      break;
    case "foldersOpened":
      void takeFolders();
      break;
    case "openFolderMenu":
      void openFolderPicker();
      break;
    case "uninstallMenu":
      openUninstall();
      break;
    case "openComputerUseSettings":
      openSettings("computerUse");
      break;
    case "settingsMenu":
      toggleSettings();
      break;
    case "feedbackMenu":
      openFeedback();
      break;
  }
}

export async function startBridge(): Promise<void> {
  startPagesFlush().catch((error: unknown) => {
    console.error("listening for the quit's page save failed", error);
  });
  startAppUpdate().catch((error: unknown) => {
    console.error("listening for a nightly update failed", error);
  });
  await subscribe(onBridgeEvent);
  startMenuBar();
  startBrowserOptions();
}
