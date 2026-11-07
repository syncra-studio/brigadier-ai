/**
 * A standalone Vite entry, excluded from the app entry and production build: a session's turn
 * Brigadier quit under mid-command, then reopened, its request going on by itself in a block of
 * its own (D84), through the real ConversationView. `?went=0`: it hasn't gone on yet (the newest
 * request still cut), so Continue picks it up. IPC is mocked, no daemon. What the thread shows
 * goes to `<pre id="quit-reopened-result">`.
 */
import { mockIPC } from "@/ipc/nativeMocks";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/conversation/ConversationView";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import type { Conversation, ConversationView as View, FileEntry, ProviderEvent, Request, ThreadTurn, ThreadView } from "@/ipc/generated";
import { openConversation } from "@/state/actions";
import { useApp } from "@/state/store";

const WENT = new URLSearchParams(location.search).get("went") !== "0";
const QUIT = "Brigadier quit while this was running.";

const ID = "quit-reopened";
const conversation = {
  id: ID, kind: "session", projectId: null, title: "X", pinnedAtMs: null, createdAtMs: 0, updatedAtMs: 0, lifecycle: "active",
  forkedFrom: null, sideOf: null, quotaWait: null,
  setup: {
    type: "session", repo: "/tmp/projX", environment: { type: "local" }, permission: "full", planMode: false,
    workersSeeUncommitted: null, orchestrator: { provider: "opencode", model: "opencode-go/glm-5.3", effort: null },
  },
} as unknown as Conversation;

const T = 1_793_005_682_000;
const at = (seconds: number, event: ProviderEvent, extra: Partial<FileEntry> = {}): FileEntry => ({ event, atMs: T + seconds * 1000, ...extra });
const turn = (userMessage: string, from: number, to: number, end: ThreadTurn["end"]): ThreadTurn => ({
  userMessage, startedAtMs: T + from * 1000, endedAtMs: T + to * 1000, end, error: null, requests: ["r1"],
});
const hold = (itemId: string, seconds: number, status: "completed" | "inProgress"): FileEntry =>
  at(seconds, { type: "command", itemId, command: "sh .smoke/hold.sh 40", cwd: null, status, exitCode: status === "completed" ? 0 : null, output: "", durationMs: null } as ProviderEvent);

const thread: ThreadView = {
  entries: [
    at(0, { type: "message", itemId: "u1", role: "user", text: "Run sh .smoke/hold.sh 40, then reply HELD." }, { said: { by: "user" }, requests: ["r1"] }),
    hold("t1", 2, "inProgress"),
    ...(WENT
      ? [
          at(30, { type: "message", itemId: "u2", role: "user", text: "[Brigadier restarted while you were working on this request, and your turn was cut off.]" }, { said: { by: "brigadier" }, requests: ["r1"] }),
          hold("t2", 32, "completed"),
          at(75, { type: "message", itemId: "a2", role: "assistant", text: "HELD" }),
        ]
      : []),
  ],
  turns: WENT ? [turn("u1", 0, 15, "interrupted"), turn("u2", 30, 75, "done")] : [turn("u1", 0, 15, "interrupted")],
};

const view = {
  conversation, context: null, messages: { messages: [], hasMore: false },
  tasks: [], approvals: [], questions: [], workerSteps: [], orchestratorSteps: [], machineSteps: [], thinking: [], compactions: [], decisions: [],
  requests: [{ id: "r1", preview: "Run sh .smoke/hold.sh 40", startedAtMs: T, endedAtMs: T + 75_000, state: WENT ? { type: "done" } : { type: "failed", error: QUIT } }],
  queue: { items: [], paused: false }, run: "idle", runRequest: null, head: null, ratings: {}, streaming: null, notices: [], previews: [],
} as unknown as View;

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "getConversation": return { method: req.method, view };
    case "threadView": return { method: req.method, view: thread };
    case "listFiles": return { method: req.method, files: [], truncated: false };
    default: return { method: req.method };
  }
});

useApp.setState({
  conversations: { [ID]: conversation },
  connection: { status: "connected", daemon: null, reason: null },
  summaryShown: { [ID]: false },
});
await openConversation(ID);

function Page() {
  const selection = useApp((s) => s.selection);
  return (
    <TooltipProvider>
      <SidebarProvider className="bg-background flex h-screen flex-col" defaultOpen={false}>
        <main className="relative flex min-h-0 w-full flex-1 flex-col">
          <ConversationView selection={selection} />
        </main>
      </SidebarProvider>
    </TooltipProvider>
  );
}
createRoot(document.getElementById("root")!).render(<Page />);

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, " ").trim() ?? null;
const blocks = () => [...document.querySelectorAll("[data-role=assistant]")];

for (let tries = 0; tries < 100 && blocks().length < (WENT ? 2 : 1); tries++) await wait(50);
await wait(300);
const result = document.createElement("pre");
result.id = "quit-reopened-result";
result.textContent = JSON.stringify({
  failed: text(document.querySelector("[data-slot=turn-render-error]")),
  headers: [...document.querySelectorAll("[data-slot=request-work-label]")].map(text),
  cards: [...document.querySelectorAll("[data-slot=error-state-summary]")].map(text),
  buttons: [...document.querySelectorAll("[data-slot=error-state] button")].map(text),
  notices: blocks().map((block) => [...block.querySelectorAll("[data-slot=task-notice]")].map(text)),
  bubbles: document.querySelectorAll("[data-role=user]").length,
});
document.body.append(result);
