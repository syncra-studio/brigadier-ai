/**
 * The thread's feedback offers in a production ConversationView: one under the finished first
 * request, one under the second, which runs unless ?done=1 (or the user stopped it, ?stopped=1).
 * Its daemon is faked: the writer can't write the first offer up (it shows as its own one
 * report) and makes the second the design's three (a bug, a repeat of #417, an idea); the filer
 * files them as @stephen-golban. ?drive=1 then reads the cards while they're written and after,
 * opens a report, opens Edit, sends the second (failing once, then Try again), dismisses the
 * first and writes what it saw to #feedback-offer-result (FeedbackOfferRender.test.ts reads
 * it). ?state=drafted|open|working|done|failed shows the second card in the design's state,
 * for screenshots.
 */
import { createRoot } from "react-dom/client";

import { FeedbackDialog } from "@/app/shell/feedback/FeedbackDialog";
import type { FeedbackFiling, FeedbackReport, OrchestratorStep } from "@/ipc/generated";
import { useBoard } from "@/state/board";
import { closeFeedback, COULDNT_REACH, useFeedbackOffers, useFeedbackUi } from "@/state/feedback";
import { useApp } from "@/state/store";

const params = new URLSearchParams(location.search);
history.replaceState(null, "", `${location.pathname}?${params.has("done") ? "view=done&" : ""}summary=0&approvals=0`);
await import("@/fixtures/flow");

// The flow fixture's daemon answers requests; the app's own commands are answered here.
const opened: string[] = [];
const wrote: string[] = [];
const REPO = "https://github.com/syncra-studio/brigadier-ai/issues";
const report = (kind: "bug" | "idea", title: string, fields: Partial<FeedbackReport>): FeedbackReport => ({
  kind,
  title,
  area: "",
  steps: "",
  expected: "",
  actual: "",
  problem: "",
  today: "",
  wanted: "",
  duplicate: null,
  ...fields,
});
const reports = [
  report("bug", "Usage popover doesn't refresh when a session ends", {
    area: "Settings, accounts, usage or updates",
    actual: "the usage popover keeps showing the old numbers after a session ends.",
    expected: "the 5-hour bar moves as soon as the session finishes.",
    steps: "run a session to the end, then open the usage popover from the sidebar.",
  }),
  report("bug", "Stop doesn't stop a running worker", {
    duplicate: {
      number: 417,
      title: "Stop leaves a worker running",
      url: `${REPO}/417`,
      open: true,
      comment: "Same here: pressing Stop on a worker doesn't stop it. It keeps writing files until it finishes on its own.",
    },
  }),
  report("idea", "Pin a chat to the top of the sidebar", {
    problem: "keep the chat I use every day at hand.",
    today: "it moves down the list as other chats get newer.",
  }),
];
/** The faked daemon: writes held until released, and every filing failing. */
const daemon = { hold: null as Promise<void> | null, failing: false };
let release = () => {};
const state = params.get("state");
if (params.has("drive") || state === "working") daemon.hold = new Promise((resolve) => (release = resolve));
const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args: unknown, options?: unknown) => Promise<unknown> } })["__TAURI_INTERNALS__"];
const invoke = internals.invoke.bind(internals);
internals.invoke = async (command, args, options) => {
  if (command === "system_name") return "macOS 27.0 Tahoe (aarch64)";
  if (command === "open_url") {
    opened.push((args as { url: string }).url);
    return null;
  }
  const request = (args as { request?: { method: string; text?: string; items?: FeedbackFiling[] } }).request;
  if (command === "ipc_request" && request?.method === "feedbackAccount") {
    return { method: request.method, signedIn: true, login: "stephen-golban" };
  }
  if (command === "ipc_request" && request?.method === "writeUpFeedback") {
    wrote.push(request.text ?? "");
    await daemon.hold;
    if (request.text?.startsWith("Typo")) throw { code: "invalid", message: "no model can write the report now" };
    return { method: request.method, writeUp: { reports, searched: true } };
  }
  if (command === "ipc_request" && request?.method === "fileFeedback") {
    return {
      method: request.method,
      signedIn: true,
      results: (request.items ?? []).map((item, index) =>
        daemon.failing
          ? { outcome: "failed", reason: "HTTP 502: Bad gateway" }
          : item.commentOn === null
            ? { outcome: "filed", number: 482 + index, url: `${REPO}/${482 + index}` }
            : { outcome: "commented", number: item.commentOn, url: `${REPO}/${item.commentOn}#issuecomment-1` },
      ),
    };
  }
  return invoke(command, args, options);
};
// Send feedback, which the app mounts beside every view.
const dialogRoot = document.createElement("div");
document.body.append(dialogRoot);
createRoot(dialogRoot).render(<FeedbackDialog />);
const calls = (window as unknown as { flow: { calls: { method: string }[] } }).flow.calls;

useApp.setState((app) => ({
  info: { ...app.info, version: "0.1.0" } as typeof app.info,
  providers: {
    ...app.providers,
    view: {
      providers: [{ provider: "claude", status: { provider: "claude", path: null, version: "2.1.4", loggedIn: true, authMethod: null, plan: null }, models: null, quota: null, usage: null, error: null, checkedAtMs: null }],
      sessions: [],
      fixtures: [],
    } as unknown as NonNullable<typeof app.providers.view>,
  },
}));
const offer = (requestId: string, position: number, id: string, title: string, body: string): OrchestratorStep => ({
  requestId,
  atMs: Date.now(),
  position,
  kind: { type: "feedbackOffered", id, kind: "bug", title, body },
});
const SECOND =
  "The usage popover keeps the old numbers after a session ends. Stop on a worker doesn't stop it. They'd like to pin a chat to the top.";
useBoard.setState(({ board }) =>
  board
    ? { board: { ...board, orchestratorSteps: [...board.orchestratorSteps, offer("r1", 7, "first", "Typo fixes take long", "A one-word fix took four minutes."), offer("r2", 30, "second", "Usage, Stop and pinning", SECOND)] } }
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
const button = (root: ParentNode | undefined, name: string) =>
  [...(root?.querySelectorAll<HTMLButtonElement>("button") ?? [])].find((each) => text(each) === name);
const rows = (root: ParentNode | undefined, list: string) => [...(root?.querySelectorAll(`[aria-label="${list}"] li`) ?? [])].map(text);
const head = (root: HTMLElement | undefined) => [...(root?.querySelectorAll("p") ?? [])].slice(0, 2).map(text);
const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

// A state of the design's, on the second card, once both are written.
if (state && state !== "working") {
  void (async () => {
    for (let tries = 0; tries < 100 && !useFeedbackOffers.getState()["second"]?.written; tries++) await wait(50);
    if (state === "open") {
      await wait(200);
      cards()[1]?.querySelector<HTMLElement>('[aria-label="Reports"] li button')?.click();
    }
    const written = useFeedbackOffers.getState()["second"]?.written ?? null;
    const results = [
      { kind: "bug" as const, title: reports[0]!.title, outcome: "filed" as const, number: 482, url: `${REPO}/482` },
      { kind: "bug" as const, title: reports[1]!.title, outcome: "added" as const, number: 417, url: `${REPO}/417` },
      { kind: "idea" as const, title: reports[2]!.title, outcome: "filed" as const, number: 483, url: `${REPO}/483` },
    ];
    if (state === "done") useFeedbackOffers.setState({ second: { written, sending: { phase: "done", results } } });
    if (state === "failed") useFeedbackOffers.setState({ second: { written, sending: { phase: "failed", error: COULDNT_REACH, results: null } } });
  })();
}

async function drive() {
  const seen: Record<string, unknown> = {};
  for (let tries = 0; tries < 100 && cards().length === 0; tries++) await wait(50);
  await wait(300);
  // While they're written: the card says so, with no buttons.
  seen.writing = cards().map((each) => [...head(each), text(each.querySelector('[data-slot="progress-line"]')), each.querySelectorAll("button").length]);
  release();
  daemon.hold = null;
  await wait(300);
  // Each card's reports, in the thread's order.
  seen.cards = cards().map((each) => rows(each, "Reports"));
  if (!params.has("done")) return seen;
  const second = () => cards()[1];
  seen.head = head(second());
  seen.buttons = [...(second()?.querySelectorAll("button") ?? [])].filter((each) => !each.closest("li")).map(text);
  second()?.querySelector<HTMLElement>('[aria-label="Reports"] li button')?.click();
  await wait(300);
  seen.report = [...(second()?.querySelectorAll('[aria-label="Reports"] li:first-child p') ?? [])].map(text);
  second()?.querySelectorAll<HTMLElement>('[aria-label="Reports"] li button')[1]?.click();
  await wait(300);
  seen.repeat = [...(second()?.querySelectorAll('[aria-label="Reports"] li:nth-child(2) p') ?? [])].map(text);
  button(second(), "Edit")?.click();
  await wait(300);
  const dialog = document.querySelector('[data-slot="feedback"]');
  seen.edit = dialog?.querySelector("textarea")?.value;
  closeFeedback();
  await wait(300);
  daemon.failing = true;
  button(second(), "Send")?.click();
  await wait(400);
  seen.failed = [text(second()?.querySelector('[role="alert"]')), [...(second()?.querySelectorAll("button") ?? [])].filter((each) => !each.closest("li")).map(text)];
  daemon.failing = false;
  button(second(), "Try again")?.click();
  await wait(400);
  seen.wrote = wrote;
  seen.opened = opened;
  // Sent, the second card says so and lists what was filed.
  seen.sent = head(second());
  seen.rows = rows(second(), "Sent");
  button(cards()[0], "Dismiss")?.click();
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
