/**
 * A standalone Vite entry, excluded from the app entry and production build: a session with a
 * plan and a rated answer, in the app's own conversation view, the plan's card beside it. IPC is
 * mocked, no daemon: a rating sent comes back on the board as the daemon's event would. It rates the plan, clears that rating, clears the answer's, and writes what it saw
 * as JSON into `<pre id="plan-rating-result">`; `stop=1` holds the page for a screenshot.
 *
 * - A plan has the answer's thumbs button: Good plan / Bad plan; once rated, Clear rating (#127).
 * - A rated answer's Feedback menu has Clear rating too.
 */
import { mockIPC } from "@tauri-apps/api/mocks";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/conversation/ConversationView";
import { PlanDocCard } from "@/app/conversation/cards/PlanDocument";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import type { Conversation, ConversationView as View, Message, Plan, Request, UserRequest } from "@/ipc/generated";
import { select } from "@/state/actions";
import { useBoard } from "@/state/board";
import { useApp } from "@/state/store";

const query = new URLSearchParams(location.search);
const id = "plan-rating";
const start = Date.now() - 20 * 60_000;
const finished = start + 20_000;

const conversation = {
  id, kind: "session", projectId: null, title: "Tax rules", pinnedAtMs: null, createdAtMs: start, updatedAtMs: finished,
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
  message("u1", 1, "user", "Move the tax rules into their own module.", start, null),
  message("a1", 200, "assistant", "The tax rules now live in `src/cart/rules.ts`.", finished, "u1"),
];
const request = {
  id: "r1", conversationId: id, preview: "Move the tax rules", state: { type: "done" }, startedAtMs: start, endedAtMs: finished,
  steeredInto: null, steeredAfter: null, undo: null, worked: [{ fromMs: start, toMs: finished }], quotaWait: false,
} as unknown as UserRequest;
const step = (title: string) => ({ title, detail: "", taskId: null, stage: "pending", startedAtMs: null, endedAtMs: null, outline: null });
const plan = {
  id: "p1", conversationId: id, requestId: null, position: 1, title: "Move the tax rules into their own module", body: "# Move the tax rules into their own module\n\nThe rules move to `src/cart/rules.ts`; checkout and basket read them there.",
  state: { type: "approved", by: "user" }, createdAtMs: start, decidedAtMs: start,
  steps: [step("Move the rules to src/cart/rules.ts"), step("Update checkout and basket"), step("Run the cart tests")],
} as unknown as Plan;

/** Each rating sent, as `subject rating`. */
const sent: string[] = [];
mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  if (req.method === "rateMessage") {
    sent.push(`${req.subject} ${req.rating ?? "cleared"}`);
    // As the daemon's event would.
    useBoard.setState((state) => {
      if (!state.board) return state;
      const { [req.subject]: _old, ...ratings } = state.board.ratings;
      return { board: { ...state.board, ratings: req.rating ? { ...ratings, [req.subject]: req.rating } : ratings } };
    });
    return { method: req.method };
  }
  if (req.method === "getConversation")
    return {
      method: req.method,
      view: {
        conversation, context: null, messages: { messages, hasMore: false }, tasks: [], approvals: [], questions: [],
        plans: [plan], requests: [request], workerSteps: [], orchestratorSteps: [], machineSteps: [], thinking: [],
        compactions: [], decisions: [], waiting: [], queue: { items: [], paused: false }, run: "idle",
        runRequest: null, head: "a1", ratings: { a1: "good" }, streaming: null, notices: [], memories: [], overnight: [],
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

const key = (target: Element, name: string) => target.dispatchEvent(new KeyboardEvent("keydown", { key: name, bubbles: true, cancelable: true }));
const openMenu = () => document.querySelector<HTMLElement>("[role=menu][data-state=open]");

/** Opens a rate button's menu (focus and Enter, as the keyboard does), says its items and picks `pick`. */
async function rateWith(button: HTMLElement, pick: string): Promise<string[]> {
  button.focus();
  key(button, "Enter");
  const menu = await until("the rate menu", openMenu);
  const items = [...menu.querySelectorAll<HTMLElement>("[role=menuitem]")];
  const words = items.map((item) => item.textContent ?? "");
  const item = items.find((each) => each.textContent === pick);
  if (!item) throw new Error(`no ${pick} in ${words.join(", ")}`);
  item.click();
  await until("the menu to close", () => !openMenu());
  return words;
}

async function drive() {
  const seen: Record<string, unknown> = {};
  try {
    const planButton = await until("the plan's rate button", () => document.querySelector<HTMLElement>("[data-slot=plan-card] [data-slot=rate-menu]"));
    seen.planLabel = planButton.querySelector(".sr-only")?.textContent;
    seen.planMenu = await rateWith(planButton, "Good plan");
    await until("the plan rated", () => planButton.dataset.rated === "good");
    seen.ratedMenu = await rateWith(planButton, "Clear rating");
    await until("the plan's rating cleared", () => planButton.dataset.rated === undefined);
    const answerButton = await until("the answer's rate button", () =>
      document.querySelector<HTMLElement>("[data-role=assistant] [data-slot=rate-menu][data-rated=good]"),
    );
    seen.answerMenu = await rateWith(answerButton, "Clear rating");
    await until("the answer's rating cleared", () => answerButton.dataset.rated === undefined);
    seen.sent = sent;
  } catch (error) {
    seen.error = String(error);
  }
  if (query.get("stop") === "1") return;
  const result = document.createElement("pre");
  result.id = "plan-rating-result";
  result.textContent = JSON.stringify(seen);
  document.body.append(result);
}

await select({ type: "conversation", id });
createRoot(document.getElementById("root")!).render(
  <TooltipProvider>
    <SidebarProvider className="bg-background flex h-screen flex-row" defaultOpen={false}>
      <main className="relative flex min-h-0 w-full flex-1 flex-col">
        <ConversationView key={id} selection={{ type: "conversation", id }} />
      </main>
      <aside className="w-90 shrink-0 p-4">
        <PlanDocCard docRef={{ type: "plan", id: plan.id }} />
      </aside>
    </SidebarProvider>
  </TooltipProvider>,
);
void drive();
