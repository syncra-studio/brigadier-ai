/** Interactive queue demo using the real conversation, queue adapter and queue card. IPC is
 * isolated in memory; delayed reorder acknowledgements mimic the daemon. */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";
import { ConversationView } from "@/app/conversation/ConversationView";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import type { Conversation, ConversationView as View, MessageQueue, Project, Request } from "@/ipc/generated";
import { select } from "@/state/actions";
import { useApp } from "@/state/store";
import { updateBoard } from "@/state/board";

const id = "queue-drag-demo";
const now = Date.now();
const conversation = {
  id, kind: "session", projectId: "p1", title: "Queue drag demo", pinnedAtMs: null,
  createdAtMs: now, updatedAtMs: now, lifecycle: "active", forkedFrom: null,
  sideOf: null, quotaWait: null, setup: null,
} as unknown as Conversation;
const initial = [
  "Review the importer changes",
  "Add retry handling to uploads",
  "Update the keyboard shortcuts",
  "Write the release notes",
  "Polish the empty state",
].map((text, index) => ({ id: `q${index + 1}`, text, attachments: [], mentions: [], queuedAtMs: now + index,
  editedAtMs: null, deciding: false, delivered: false }));
let queue: MessageQueue = { items: [...initial], paused: true };
const calls: Request[] = [];
mockIPC(async (command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  calls.push(req);
  switch (req.method) {
    case "getConversation":
      return { method: req.method, view: {
        conversation, context: null, messages: { messages: [], hasMore: false }, tasks: [], approvals: [], questions: [],
        requests: [], workerSteps: [], orchestratorSteps: [], machineSteps: [], thinking: [], compactions: [], decisions: [],
        queue, run: "idle", runRequest: null, head: null, ratings: {}, streaming: null, notices: [], previews: [],
      } as unknown as View };
    case "threadView":
      return { method: req.method, view: { entries: [], turns: [], workers: [] } };
    case "moveQueued": {
      const at = queue.items.findIndex((item) => item.id === req.itemId);
      const items = [...queue.items];
      const [item] = items.splice(at, 1);
      if (item) items.splice(req.index, 0, item);
      queue = { ...queue, items };
      await new Promise((resolve) => setTimeout(resolve, 250));
      return { method: req.method, queue };
    }
    case "deleteQueued": {
      const items = [...queue.items];
      const at = items.findIndex((item) => item.id === req.itemId);
      if (at >= 0) items.splice(at, 1);
      queue = { ...queue, items };
      return { method: req.method, queue };
    }
    case "listFiles": return { method: req.method, files: [], truncated: false };
    default: return { method: req.method };
  }
});
const state = useApp.getState();
useApp.setState({
  conversations: { [id]: conversation },
  projects: { p1: { id: "p1", name: "Queue demo", createdAtMs: now, repos: [{ path: "/tmp/queue-demo", name: "queue-demo" }], prefs: {}, trust: [] } as unknown as Project },
  info: { ...state.info, platform: "macos" } as typeof state.info,
  connection: { status: "connected", daemon: null, reason: null },
});
// Available to the later regression render: publish an external reorder while the pointer is
// still held, reproducing the optimistic/daemon update that used to duplicate keyed rows.
const demo = {
  calls,
  reorderWhileDragging: () => {
    queue = { ...queue, items: [...queue.items.slice(1), queue.items[0]!] };
    updateBoard(id, (board) => ({ ...board, queue }));
  },
  reset: () => {
    queue = { ...queue, items: [...initial] };
    updateBoard(id, (board) => ({ ...board, queue }));
  },
};
Object.assign(window, { queueDragDemo: demo });
await select({ type: "conversation", id });
createRoot(document.getElementById("root")!).render(
  <TooltipProvider>
    <SidebarProvider className="bg-background flex h-screen flex-col" defaultOpen={false}>
      <div className="border-border border-b px-6 py-4 text-sm">
        <strong>Queue drag demo</strong> — Hover a row and drag its grip. Delete removes one message. Reload to reset.
      </div>
      <main className="relative flex min-h-0 w-full flex-1 flex-col">
        <ConversationView selection={{ type: "conversation", id }} />
      </main>
    </SidebarProvider>
  </TooltipProvider>,
);

if (new URLSearchParams(location.search).has("drive")) {
  const { driveQueue } = await import("@/fixtures/queue-drag-drive");
  void driveQueue(demo);
}
