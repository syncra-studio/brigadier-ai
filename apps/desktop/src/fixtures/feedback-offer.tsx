/**
 * The thread's feedback offers in a production ConversationView: one under the finished first
 * request, one under the second, which runs unless ?done=1 (or the user stopped it, ?stopped=1).
 * ?drive=1 then reads the cards,
 * opens Edit first, sends one, dismisses the other and writes what it saw to
 * #feedback-offer-result (FeedbackOfferRender.test.ts reads it).
 */
import { createRoot } from "react-dom/client";

import { FeedbackDialog } from "@/app/shell/feedback/FeedbackDialog";
import type { OrchestratorStep } from "@/ipc/generated";
import { useBoard } from "@/state/board";
import { closeFeedback, useFeedbackUi } from "@/state/feedback";
import { useApp } from "@/state/store";

const params = new URLSearchParams(location.search);
history.replaceState(null, "", `${location.pathname}?${params.has("done") ? "view=done&" : ""}summary=0&approvals=0`);
await import("@/fixtures/flow");

// The flow fixture's daemon answers requests; the app's own commands are answered here.
const opened: string[] = [];
const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args: unknown, options?: unknown) => Promise<unknown> } })["__TAURI_INTERNALS__"];
const invoke = internals.invoke.bind(internals);
internals.invoke = (command, args, options) => {
  if (command === "system_name") return Promise.resolve("macOS 27.0 Tahoe (aarch64)");
  if (command === "open_url") {
    opened.push((args as { url: string }).url);
    return Promise.resolve(null);
  }
  return invoke(command, args, options);
};
// Send feedback, which the app mounts beside every view.
const dialogRoot = document.createElement("div");
document.body.append(dialogRoot);
createRoot(dialogRoot).render(<FeedbackDialog />);
const calls = (window as unknown as { flow: { calls: { method: string }[] } }).flow.calls;

useApp.setState((state) => ({
  info: { ...state.info, version: "0.1.0" } as typeof state.info,
  providers: {
    ...state.providers,
    view: {
      providers: [{ provider: "claude", status: { provider: "claude", path: null, version: "2.1.4", loggedIn: true, authMethod: null, plan: null }, models: null, quota: null, usage: null, error: null, checkedAtMs: null }],
      sessions: [],
      fixtures: [],
    } as unknown as NonNullable<typeof state.providers.view>,
  },
}));
const offer = (requestId: string, position: number, id: string, title: string): OrchestratorStep => ({
  requestId,
  atMs: Date.now(),
  position,
  kind: { type: "feedbackOffered", id, kind: "bug", title, body: "After I press Stop mid-reply, the tab keeps its spinner." },
});
useBoard.setState(({ board }) =>
  board
    ? { board: { ...board, orchestratorSteps: [...board.orchestratorSteps, offer("r1", 7, "first", "Typo fixes take long"), offer("r2", 30, "second", "Stop leaves the tab spinning")] } }
    : {},
);
if (params.has("stopped")) {
  useBoard.setState(({ board }) => {
    const request = board?.requests.r2;
    return board && request ? { board: { ...board, requests: { ...board.requests, r2: { ...request, state: { type: "stopped" as const }, endedAtMs: Date.now() } } } } : {};
  });
}

const text = (element: Element | null | undefined) => element?.textContent?.trim() ?? "";
const cards = () => [...document.querySelectorAll<HTMLElement>('[data-kind="feedbackOffered"]')];
const card = (title: string) => cards().find((each) => text(each).includes(title));
const button = (root: ParentNode | undefined, name: string) =>
  [...(root?.querySelectorAll<HTMLButtonElement>("button") ?? [])].find((each) => text(each) === name);
const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

async function drive() {
  const seen: Record<string, unknown> = {};
  for (let tries = 0; tries < 100 && cards().length === 0; tries++) await wait(50);
  await wait(300);
  // Each card's draft title, in the thread's order.
  seen.cards = cards().map((each) => text(each.querySelector("[class*=rounded-settings] p")));
  if (!params.has("done")) return seen;
  const sending = card("Stop leaves the tab spinning");
  seen.details = text([...(sending?.querySelectorAll("p") ?? [])].find((p) => text(p).startsWith("Adds")));
  seen.buttons = [...(sending?.querySelectorAll("button") ?? [])].map((each) => text(each));
  button(sending, "Edit first")?.click();
  await wait(300);
  const dialog = document.querySelector('[data-slot="feedback"]');
  seen.edit = {
    text: dialog?.querySelector("textarea")?.value,
    kind: text(dialog?.querySelector('[role="radio"][aria-checked="true"]')),
  };
  closeFeedback();
  await wait(300);
  button(card("Stop leaves the tab spinning"), "Open on GitHub")?.click();
  await wait(300);
  seen.opened = opened.map((url) => Object.fromEntries(new URL(url).searchParams));
  // Sent, the second card is its one line.
  seen.sent = text(cards()[1]?.querySelector('[role="status"]'));
  button(card("Typo fixes take long"), "Dismiss")?.click();
  await wait(300);
  seen.left = cards().length;
  seen.answers = calls.filter((call) => call.method === "answerFeedbackOffer");
  seen.dialogOpen = useFeedbackUi.getState().open;
  return seen;
}

if (params.has("drive")) {
  void drive().then((seen) => {
    const result = document.createElement("pre");
    result.id = "feedback-offer-result";
    result.textContent = JSON.stringify(seen);
    document.body.append(result);
  });
}
