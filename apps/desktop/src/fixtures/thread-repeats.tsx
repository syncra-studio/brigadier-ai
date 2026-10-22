/**
 * A standalone Vite entry, excluded from the app entry and production build: a session whose one
 * turn called a tool Brigadier has no words for six times in a row, in the app's own conversation
 * view. IPC is mocked, no daemon. It opens the turn's work, its group and the fold, and writes
 * what it saw as JSON into `<pre id="thread-repeats-result">`; `stop=1` holds the page for a
 * screenshot.
 *
 * - The group's summary counts the calls; opened, the six are one row with their count and time;
 *   that row opens to each call with what it was given (#101). Two calls of another tool stay
 *   two rows.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/conversation/ConversationView";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import type { Conversation, ConversationView as View, Message, OrchestratorStep, Request, UserRequest } from "@/ipc/generated";
import { select } from "@/state/actions";
import { useApp } from "@/state/store";

const query = new URLSearchParams(location.search);
const id = "thread-repeats";
const MINUTE = 60_000;
const start = Date.now() - 20 * MINUTE;
const finished = start + 60_000;

const conversation = {
  id, kind: "session", projectId: null, title: "Checkout tax", pinnedAtMs: null, createdAtMs: start, updatedAtMs: finished,
  lifecycle: "active", forkedFrom: null, sideOf: null, fallback: null, quotaWait: null,
  setup: {
    type: "session", orchestrator: { provider: "claude", model: null, effort: null, fast: false }, permission: "askForApproval", planMode: false,
    workersSeeUncommitted: null, environment: { type: "localCheckout", repoPath: "/repo", branch: null },
  },
} as unknown as Conversation;

const message = (messageId: string, seq: number, role: "user" | "assistant", text: string, createdAtMs: number, parentId: string | null): Message => ({
  id: messageId, conversationId: id, seq, role, text, blob: null, createdAtMs, attachments: [], mentions: [], model: null, requestId: "r1", parentId,
});
const messages: Message[] = [
  message("u1", 1, "user", "Find the open issues about checkout tax and fix the bug they describe.", start, null),
  message("a1", 200, "assistant", "Fixed LIN-412: tax was rounded per line instead of on the basket total.", finished, "u1"),
];
const call = (position: number, name: string, detail: string, atMs: number): OrchestratorStep => ({
  requestId: "r1", atMs, position,
  kind: { type: "tool", itemId: `call-${position}`, name, detail, status: "completed", throughPosition: position, endedAtMs: atMs + 2000 },
});
const queries = ["checkout tax", "tax rounding", "VAT basket", "tax EU", "discount tax", "tax regression"];
const steps: OrchestratorStep[] = [
  call(2, "Read", "/repo/src/cart/tax.ts", start + 2000),
  ...queries.map((detail, index) => call(3 + index, "mcp__linear__search_issues", detail, start + 5000 + index * 3000)),
  call(9, "mcp__linear__get_issue", "LIN-412", start + 25_000),
  call(10, "mcp__linear__get_issue", "LIN-413", start + 28_000),
  call(11, "Edit", "/repo/src/cart/tax.ts", start + 40_000),
];
const request = {
  id: "r1", conversationId: id, preview: "Find the open issues", state: { type: "done" }, startedAtMs: start, endedAtMs: finished,
  steeredInto: null, steeredAfter: null, undo: null, worked: [{ fromMs: start, toMs: finished }], quotaWait: false,
} as unknown as UserRequest;

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "getConversation")
    return {
      method: req.method,
      view: {
        conversation, context: null, messages: { messages, hasMore: false }, tasks: [], approvals: [], questions: [],
        plans: [], requests: [request], workerSteps: [], orchestratorSteps: steps, machineSteps: [], thinking: [],
        compactions: [], decisions: [], waiting: [], queue: { items: [], paused: false }, run: "idle",
        runRequest: null, head: "a1", ratings: {}, streaming: null, notices: [], memories: [], overnight: [],
        previews: [],
      } as unknown as View,
    };
  return { method: req.method };
});
useApp.setState({ conversations: { [id]: conversation }, connection: { status: "connected", daemon: null, reason: null } });

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
/** Waits for an element (never a guess at timing), failing with what it waited for. */
async function until<T>(what: string, find: () => T | null | undefined, ms = 10_000): Promise<T> {
  const end = performance.now() + ms;
  for (;;) {
    const found = find();
    if (found) return found;
    if (performance.now() > end) throw new Error(`timed out waiting for ${what}`);
    await sleep(16);
  }
}

/** Each row of a list, its own line only (an opened row's calls are not its words), its parts spaced. */
const rows = (parent: HTMLElement) =>
  [...parent.children].map((row) => {
    const line = (row.matches("[role=button]") ? row : row.querySelector(":scope > [role=button]")) ?? row;
    return [...line.children].map((part) => part.textContent).filter(Boolean).join(" ");
  });

async function drive() {
  const seen: Record<string, unknown> = {};
  try {
    const turn = await until("the turn", () => document.querySelector<HTMLElement>("[data-role=assistant]"));
    const header = await until("the work header", () => turn.querySelector<HTMLElement>("[data-slot=request-work-header]"));
    if (header.getAttribute("aria-expanded") === "false") header.click();
    const work = await until("the opened work", () => turn.querySelector<HTMLElement>("[data-slot=request-work]"));
    const group = await until("the group", () => work.querySelector<HTMLElement>("[data-slot=work-group]"));
    const toggle = group.querySelector<HTMLElement>(":scope > button")!;
    seen.summary = toggle.textContent;
    toggle.click();
    const list = await until("the opened group", () => group.querySelector<HTMLElement>("[data-slot=work-group-list]"));
    seen.rows = rows(list);
    const fold = await until("the fold", () => list.querySelector<HTMLElement>("[data-slot=repeats]"));
    fold.querySelector<HTMLElement>(":scope > [role=button]")!.click();
    const calls = await until("the fold's calls", () => fold.querySelector<HTMLElement>(":scope > [data-state=open]:not([role=button]) > div"));
    seen.calls = rows(calls);
  } catch (error) {
    seen.error = String(error);
  }
  if (query.get("stop") === "1") return;
  const result = document.createElement("pre");
  result.id = "thread-repeats-result";
  result.textContent = JSON.stringify(seen);
  document.body.append(result);
}

await select({ type: "conversation", id });
createRoot(document.getElementById("root")!).render(
  <TooltipProvider>
    <SidebarProvider className="bg-background flex h-screen flex-col" defaultOpen={false}>
      <main className="relative flex min-h-0 w-full flex-1 flex-col">
        <ConversationView key={id} selection={{ type: "conversation", id }} />
      </main>
    </SidebarProvider>
  </TooltipProvider>,
);
void drive();
