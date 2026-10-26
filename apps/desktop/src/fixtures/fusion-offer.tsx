import { fusionProviders } from "@/fixtures/fusionProviders";
import type { OrchestratorStep } from "@/ipc/generated";
import { useBoard } from "@/state/board";
import { useApp } from "@/state/store";

const params = new URLSearchParams(location.search);
history.replaceState(null, "", `${location.pathname}?summary=0&approvals=0`);
await import("@/fixtures/flow");

const calls = (window as unknown as { flow: { calls: { method: string }[] } }).flow.calls;
const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args: unknown, options?: unknown) => Promise<unknown> } })["__TAURI_INTERNALS__"];
const invoke = internals.invoke.bind(internals);
// A user click records consent and sends the question in this Chat.
internals.invoke = (command, args, options) => {
  const request = (args as { request?: { method: string; conversationId: string; offerId: string } } | undefined)?.request;
  if (command !== "ipc_request" || request?.method !== "startOfferedFusion") return invoke(command, args, options);
  calls.push(request);
  useBoard.setState(({ board }) => board ? { board: { ...board, orchestratorSteps: [...board.orchestratorSteps, { requestId: "r1", atMs: Date.now(), position: 8, kind: { type: "fusionOfferTaken", id: request.offerId } }] } } : {});
  return Promise.resolve({ method: "startOfferedFusion", outcome: { type: "queued", item: {} } });
};

const offer = (requestId: string, position: number, id: string, title: string): OrchestratorStep => ({
  requestId,
  atMs: Date.now(),
  position,
  kind: { type: "fusionOffered", id, question: title },
});
useApp.setState((state) => ({
  conversations: Object.fromEntries(Object.entries(state.conversations).map(([id, conversation]) => [id, { ...conversation, kind: "chat", projectId: null, setup: { type: "chat", model: { provider: "codex", model: null, effort: null, fast: false }, permission: "acceptEdits" } }])),
  providers: { ...state.providers, view: { providers: fusionProviders, sessions: [], fixtures: [] } },
}));
useBoard.setState(({ board }) =>
  board
    ? { board: { ...board, orchestratorSteps: [...board.orchestratorSteps, offer("r1", 7, "first", "Fix the flaky upload test"), offer("r2", 30, "second", "Rename the settings keys")] } }
    : {},
);

const text = (element: Element | null | undefined) => element?.textContent?.trim() ?? "";
const cards = () => [...document.querySelectorAll<HTMLElement>('[data-kind="fusionOffered"]')];
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
  seen.cards = cards().map((each) => text(each.querySelector("[class*=rounded-settings] p:last-child")));
  seen.places = cards().map((each) => (each.closest('[data-slot="request-work"]') ? "work" : "kept"));
  seen.heading = text(cards()[0]?.querySelector("p"));
  seen.buttons = [...(cards()[0]?.querySelectorAll("button") ?? [])].map((each) => text(each));
  seen.ignoredCalls = calls.filter((call) => call.method === "startOfferedFusion").length;
  const shown = useApp.getState().selection;
  button(cards()[0], "Ask")?.click();
  await until(() => text(cards()[0]?.querySelector("p")) === "Asked");
  seen.started = text(cards()[0]?.querySelector("p"));
  seen.startedButtons = [...(cards()[0]?.querySelectorAll("button") ?? [])].map((each) => text(each));
  seen.calls = calls.filter((call) => call.method === "startOfferedFusion").map((call) => ({ ...call, conversationId: undefined }));
  seen.stayed = JSON.stringify(useApp.getState().selection) === JSON.stringify(shown);
  useApp.setState((state) => ({ providers: { ...state.providers, view: { providers: fusionProviders.slice(0, 1), sessions: [], fixtures: [] } } }));
  await until(() => !!button(cards()[1], "Ask")?.disabled);
  seen.disabled = button(cards()[1], "Ask")?.disabled;
  seen.hint = cards()[1]?.textContent?.includes("Needs two ready CLIs");
  const cid = useBoard.getState().board!.conversationId;
  useApp.setState((state) => ({ conversations: { ...state.conversations, [cid]: { ...state.conversations[cid]!, kind: "session" } } }));
  await until(() => cards().length === 0);
  seen.sessionCards = cards().length;
  return seen;
}

if (params.has("drive")) {
  void drive().then((seen) => {
    const result = document.createElement("pre");
    result.id = "fusion-offer-result";
    result.textContent = JSON.stringify(seen);
    document.body.append(result);
  });
}
