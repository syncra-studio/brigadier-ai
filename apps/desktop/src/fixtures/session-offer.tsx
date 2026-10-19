/**
 * The thread's session offers in a production ConversationView: one under the finished first
 * request, one under the second, which still runs. ?drive=1 reads the cards, clicks Start as
 * session on the first, then Open, and writes what it saw to #session-offer-result
 * (SessionOfferRender.test.ts reads it).
 */
import type { Conversation, OrchestratorStep } from "@/ipc/generated";
import { useBoard } from "@/state/board";
import { useApp } from "@/state/store";

const params = new URLSearchParams(location.search);
history.replaceState(null, "", `${location.pathname}?summary=0&approvals=0`);
await import("@/fixtures/flow");

const calls = (window as unknown as { flow: { calls: { method: string }[] } }).flow.calls;
const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args: unknown, options?: unknown) => Promise<unknown> } })["__TAURI_INTERNALS__"];
const invoke = internals.invoke.bind(internals);
// The daemon starts the offered task as a new session and records that the offer was taken.
internals.invoke = (command, args, options) => {
  const request = (args as { request?: { method: string; conversationId: string; offerId: string } } | undefined)?.request;
  if (command !== "ipc_request" || request?.method !== "startOfferedSession") return invoke(command, args, options);
  calls.push(request);
  const source = useApp.getState().conversations[request.conversationId] as Conversation;
  const conversation = { ...source, id: "offered-session", title: "Fix the flaky upload test" };
  useBoard.setState(({ board }) =>
    board
      ? { board: { ...board, orchestratorSteps: [...board.orchestratorSteps, { requestId: "r1", atMs: Date.now(), position: 8, kind: { type: "sessionOfferTaken", id: request.offerId, session: conversation.id } }] } }
      : {},
  );
  return Promise.resolve({ method: "startOfferedSession", conversation, outcome: { type: "sent" } });
};

const offer = (requestId: string, position: number, id: string, title: string): OrchestratorStep => ({
  requestId,
  atMs: Date.now(),
  position,
  kind: { type: "sessionOffered", id, title, task: `${title}: it fails about one run in ten on CI, outside what this session changes.` },
});
useBoard.setState(({ board }) =>
  board
    ? { board: { ...board, orchestratorSteps: [...board.orchestratorSteps, offer("r1", 7, "first", "Fix the flaky upload test"), offer("r2", 30, "second", "Rename the settings keys")] } }
    : {},
);

const text = (element: Element | null | undefined) => element?.textContent?.trim() ?? "";
const cards = () => [...document.querySelectorAll<HTMLElement>('[data-kind="sessionOffered"]')];
const button = (root: ParentNode | undefined, name: string) =>
  [...(root?.querySelectorAll<HTMLButtonElement>("button") ?? [])].find((each) => text(each) === name);
const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(check: () => boolean) {
  for (let tries = 0; tries < 100 && !check(); tries++) await wait(50);
}

async function drive() {
  const seen: Record<string, unknown> = {};
  await until(() => cards().length > 0);
  await wait(300);
  // Each card's title, in the thread's order: the running request's shows too, in place.
  seen.cards = cards().map((each) => text(each.querySelector("[class*=rounded-settings] p")));
  // Where each sits: in its request's work while it runs, kept in view once it is done.
  seen.places = cards().map((each) => (each.closest('[data-slot="request-work"]') ? "work" : "kept"));
  seen.heading = text(cards()[0]?.querySelector("p"));
  seen.buttons = [...(cards()[0]?.querySelectorAll("button") ?? [])].map((each) => text(each));
  const shown = useApp.getState().selection;
  button(cards()[0], "Start as session")?.click();
  await until(() => text(cards()[0]?.querySelector("p")) === "Started as a session");
  seen.started = text(cards()[0]?.querySelector("p"));
  seen.startedButtons = [...(cards()[0]?.querySelectorAll("button") ?? [])].map((each) => text(each));
  seen.calls = calls.filter((call) => call.method === "startOfferedSession" || call.method === "startSession").map((call) => ({ ...call, conversationId: undefined }));
  // The user stayed where they were.
  seen.stayed = JSON.stringify(useApp.getState().selection) === JSON.stringify(shown);
  seen.stored = !!useApp.getState().conversations["offered-session"];
  button(cards()[0], "Open")?.click();
  await until(() => JSON.stringify(useApp.getState().selection).includes("offered-session"));
  seen.opened = useApp.getState().selection;
  return seen;
}

if (params.has("drive")) {
  void drive().then((seen) => {
    const result = document.createElement("pre");
    result.id = "session-offer-result";
    result.textContent = JSON.stringify(seen);
    document.body.append(result);
  });
}
