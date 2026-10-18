/**
 * A standalone Vite entry, excluded from the app entry and production build: the composer's
 * small behaviours in the app's own conversation view, one case per page load (`?case=`). A Chat
 * whose one request is over; IPC is mocked and records what the composer sends. It drives the
 * real keys (`fixtureInput`) and writes what it saw as JSON into `<pre id="composer-polish-result">`.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/ConversationView";
import { SidebarProvider } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { Conversation, ConversationView as View, Message, Request, UserRequest } from "@/ipc/generated";
import { fixtureClick, fixtureInput } from "@/fixtures/input";
import { select } from "@/state/actions";
import { useBoard } from "@/state/board";
import { useApp } from "@/state/store";

const id = "polish-chat";
const start = Date.now() - 600_000;
const which = new URLSearchParams(location.search).get("case");
// A draft for the cases that start with one (the editor then has text to put its caret in).
localStorage.clear();
if (which === "never") {
  localStorage.setItem("brigadier.drafts", JSON.stringify({ [id]: { text: "half a thought", attachments: [], mentions: [], atMs: Date.now() } }));
}

const conversation = {
  id, kind: "chat", projectId: null, title: "A chat", pinnedAtMs: null, createdAtMs: start,
  updatedAtMs: start, lifecycle: "active", forkedFrom: null, sideOf: null, fallback: null, quotaWait: null,
  setup: { type: "chat", model: { provider: "claude", model: null, effort: null, fast: false } },
} as unknown as Conversation;
const messages: Message[] = [
  { id: "u1", conversationId: id, seq: 1, role: "user", text: "Fix the failing test.", blob: null, createdAtMs: start,
    attachments: [], mentions: [], model: null, requestId: "r1", parentId: null },
  { id: "a1", conversationId: id, seq: 2, role: "assistant", text: "Fixed: the date was parsed in the wrong zone.",
    blob: null, createdAtMs: start + 30_000, attachments: [], mentions: [], model: null, requestId: "r1", parentId: "u1" },
];
const request = {
  id: "r1", conversationId: id, preview: "Fix the failing test.", state: { type: "done" }, startedAtMs: start,
  endedAtMs: start + 30_000, steeredInto: null, steeredAfter: null, undo: null, worked: [], quotaWait: false,
  suggestion: null,
} as unknown as UserRequest;

/** What the composer sent, by method. */
const sent: Request[] = [];
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "getConversation":
      return {
        method: req.method,
        view: {
          conversation, context: null, messages: { messages, hasMore: false }, tasks: [], approvals: [], questions: [],
          plans: [], requests: [request], workerSteps: [], orchestratorSteps: [], machineSteps: [], thinking: [],
          compactions: [], decisions: [], waiting: [], queue: { items: [], paused: false }, run: "idle",
          runRequest: null, head: "a1", ratings: {}, streaming: null, notices: [], memories: [], overnight: [],
          reviews: [], previews: [],
        } as unknown as View,
      };
    default:
      sent.push(req);
      return { method: req.method };
  }
});
useApp.setState({ conversations: { [id]: conversation }, connection: { status: "connected", daemon: null, reason: null } });

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
/** Waits for a condition (never a guess at timing), failing with what it waited for. */
async function until(what: string, check: () => boolean, ms = 10_000) {
  const end = performance.now() + ms;
  while (!check()) {
    if (performance.now() > end) throw new Error(`timed out waiting for ${what}`);
    await sleep(16);
  }
}
const field = () => document.querySelector<HTMLElement>("[data-slot=aui_composer-shell] [contenteditable=true]");

/** The latest request's state, as the daemon would publish it. */
const setState = (state: UserRequest["state"]) =>
  useBoard.setState(({ board }) =>
    board ? { board: { ...board, requests: { ...board.requests, r1: { ...board.requests["r1"]!, state } } } } : {},
  );

/** What has the keyboard, by its label. */
const focused = () => document.activeElement?.getAttribute("aria-label") ?? document.activeElement?.tagName ?? null;

/**
 * #304: a field the user went to (Find in the chat) keeps the keyboard when the composer would
 * focus itself, as it does when its editor loads and when a run starts.
 */
async function focusCase(seen: Record<string, unknown>) {
  await until("the editor", () => field() !== null);
  await fixtureClick(field()!);
  await fixtureInput({ type: "type", text: "half" });
  await until("the typed text", () => field()?.textContent === "half");
  const find = document.createElement("input");
  find.setAttribute("aria-label", "Find in chat");
  document.body.append(find);
  find.focus();
  setState({ type: "working" });
  await sleep(300);
  seen.runStart = focused();
  setState({ type: "done" });
  // With nothing else focused, a run's start still brings the keyboard back to the composer.
  find.remove();
  await sleep(100);
  setState({ type: "working" });
  await until("the composer focused", () => focused() === "Message input", 3000).catch(() => {});
  seen.runStartFree = focused();
  setState({ type: "done" });
}

const CASES: Record<string, (seen: Record<string, unknown>) => Promise<void>> = {
  focus: focusCase,
};

async function drive() {
  const seen: Record<string, unknown> = {};
  try {
    const run = CASES[which ?? ""];
    if (!run) throw new Error(`no case ${String(which)}`);
    await run(seen);
    seen.sent = sent.map((req) => req.method);
  } catch (error) {
    seen.error = String(error);
  }
  const result = document.createElement("pre");
  result.id = "composer-polish-result";
  result.textContent = JSON.stringify(seen);
  document.body.append(result);
}

select({ type: "conversation", id });
void drive();
createRoot(document.getElementById("root")!).render(
  <TooltipProvider>
    <SidebarProvider className="bg-background flex h-screen flex-col" defaultOpen={false}>
      <main className="relative flex min-h-0 w-full flex-1 flex-col">
        <ConversationView key={id} selection={{ type: "conversation", id }} />
      </main>
    </SidebarProvider>
  </TooltipProvider>,
);
