import { subscribe } from "@/ipc/client";
import type { BridgeEvent, EventEnvelope } from "@/ipc/generated";
import { markApplied, noteFlush, setSamplingPaused } from "@/lib/perf";
import { markStartup } from "@/lib/startup";
import {
  loadCatalog,
  loadConversation,
  openConversation,
  openOrchestratorLog,
  toggleSettings,
} from "@/state/actions";
import { applyActivityEvents, loadActivity } from "@/state/activity";
import { openFolderPicker, takeFolders } from "@/state/addProject";
import { applyBoardEvents, sideBoardIds, useBoard } from "@/state/board";
import { applyBrainEvents } from "@/state/brain";
import { applyEvents, useApp } from "@/state/store";
import { startMenuBar } from "@/state/menuBar";
import { applyUpdateEvents, loadUpdates } from "@/state/updates";
import { applyUsageEvents } from "@/state/usage";
import { onDictationDisconnected, onDictationUpdate } from "@/state/dictation";
import { openUninstall } from "@/state/storage";
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
  applyBoardEvents(batch);
  applyActivityEvents(batch);
  applyBrainEvents(batch);
  applyUsageEvents(batch);
  applyUpdateEvents(batch);
  noteFlush(started, batch.map(({ event }) => event.type));
  for (const { atMs, event } of batch) {
    markApplied(atMs, event.type === "probe" ? event.burstId : null);
  }
}

/** Reloads everything the UI holds: used after (re)connecting and after missing events.
 * Threads other than the shown ones may be stale, so they are dropped and reload when opened.
 * The shown conversations' boards and the Inspector's orchestrator log are read again. */
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
  await Promise.all([loadCatalog(), loadActivity(), loadUpdates()]);
  const log = useBoard.getState().orchestrator;
  if (log) void openOrchestratorLog(log.conversationId);
  await Promise.all(shown.map((id) => loadConversation(id)));
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
    case "settingsMenu":
      toggleSettings();
      break;
  }
}

export async function startBridge(): Promise<void> {
  await subscribe(onBridgeEvent);
  startMenuBar();
}
