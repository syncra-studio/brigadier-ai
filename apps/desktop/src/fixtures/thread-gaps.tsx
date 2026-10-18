/**
 * A standalone Vite entry, excluded from the app entry and production build: a session whose one
 * turn paused for more than two hours between its steps, in the app's own conversation view. IPC
 * is mocked, no daemon. It opens the turn's work and writes what it saw as JSON into
 * `<pre id="thread-gaps-result">`; `stop=1` holds the page for a screenshot.
 *
 * - The pause: a "time passed" line inside the turn, at the time the work went on (#130), and the
 *   work around it in two groups.
 * - `scene=quota`: the pause was a usage limit that reset; a divider, "Claude usage limit reset ·
 *   9:39 PM", stands there instead of the time line (#128).
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/ConversationView";
import { SidebarProvider } from "@/components/ui/sidebar";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { Conversation, ConversationView as View, Message, OrchestratorStep, Request, UserRequest } from "@/ipc/generated";
import { formatDaySeparator, formatTime } from "@/lib/format";
import { select } from "@/state/actions";
import { useApp } from "@/state/store";

const query = new URLSearchParams(location.search);
const quota = query.get("scene") === "quota";
const id = "thread-gaps";
const MINUTE = 60_000;
const start = Date.now() - 5 * 60 * MINUTE;
/** When the turn went on after its pause. */
const back = start + 134 * MINUTE;

const conversation = {
  id, kind: "session", projectId: null, title: "Tax rules", pinnedAtMs: null, createdAtMs: start, updatedAtMs: back,
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
  message("u1", 1, "user", "Move the tax rules into their own module and update the callers.", start, null),
  message("a1", 200, "assistant", "The tax rules now live in `src/cart/rules.ts`. Tests pass.", back + 40_000, "u1"),
];
const call = (position: number, name: string, detail: string, atMs: number): OrchestratorStep => ({
  requestId: "r1", atMs, position,
  kind: { type: "tool", itemId: `call-${position}`, name, detail, status: "completed", throughPosition: position, endedAtMs: atMs + 2000 },
});
const steps: OrchestratorStep[] = [
  call(2, "Read", "/repo/src/cart/tax.ts", start + 5000),
  call(3, "Grep", "applyTax", start + 9000),
  call(4, "Edit", "/repo/src/cart/rules.ts", start + 20_000),
  // The limit it waited for reset.
  ...(quota ? [{ requestId: "r1", atMs: back - 5000, position: 5, kind: { type: "quotaResumed", provider: "claude" } } as OrchestratorStep] : []),
  // After the pause.
  call(6, "Edit", "/repo/src/cart/checkout.ts", back),
  call(7, "Bash", "pnpm test cart", back + 20_000),
];
const request = {
  id: "r1", conversationId: id, preview: "Move the tax rules", state: { type: "done" }, startedAtMs: start, endedAtMs: back + 42_000,
  steeredInto: null, steeredAfter: null, undo: null, worked: [{ fromMs: start, toMs: back + 42_000 }], quotaWait: false,
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
        reviews: [], previews: [],
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

async function drive() {
  const seen: Record<string, unknown> = {};
  try {
    const turn = await until("the turn", () => document.querySelector<HTMLElement>("[data-role=assistant]"));
    const header = await until("the work header", () => turn.querySelector<HTMLElement>("[data-slot=request-work-header]"));
    if (header.getAttribute("aria-expanded") === "false") header.click();
    const work = await until("the opened work", () => turn.querySelector<HTMLElement>("[data-slot=request-work]"));
    await until("the line", () => work.querySelector("[data-slot=time-separator], [data-slot=quota-resumed]"));
    // What the opened work shows, in order: its groups by their summary, and the line.
    seen.work = [...work.querySelectorAll<HTMLElement>("[data-slot=work-group] > button, [data-slot=time-separator], [data-slot=quota-resumed]")].map((element) =>
      element.dataset.slot === "time-separator" ? `line: ${element.textContent}`
        : element.dataset.slot === "quota-resumed" ? `divider: ${element.textContent}`
          : `group: ${element.textContent}`,
    );
    seen.expected = quota ? formatTime(back - 5000) : formatDaySeparator(back, Date.now());
  } catch (error) {
    seen.error = String(error);
  }
  if (query.get("stop") === "1") return;
  const result = document.createElement("pre");
  result.id = "thread-gaps-result";
  result.textContent = JSON.stringify(seen);
  document.body.append(result);
}

select({ type: "conversation", id });
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
