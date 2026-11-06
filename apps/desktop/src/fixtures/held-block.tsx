/**
 * A standalone Vite entry, excluded from the app entry and production build: a session's block
 * held by its workers. Its lead starts a worker and ends its turn; the user writes twice while
 * the workers work (the second turn starts another), each message a turn that joins the block;
 * then the workers finish one by one, and their word joins it too. Each step's work headers
 * and blocks are read into `#held-block-result`.
 *
 * `?failed=wake`, `?failed=nothing`: its lead's turn fails while its worker works; the worker's
 * end then wakes the thread, or nothing follows it. Each step reads the same way, with its error.
 *
 * `?refresh=wake`, `?refresh=quiet`: its lead's turn ends, Brigadier refreshes its context while two
 * workers work; a worker's end then wakes the thread as a real session's does, a hidden input that
 * opens no turn, so its work goes on under the `/compact`; or nothing follows it.
 *
 * `?seam=held`: its lead's turn ends and Brigadier hands the session on while its workers work: a
 * hidden `/compact`, a hidden note turn, then a successor that carries on by itself and messages a
 * worker, its turn joining the block; its turn ends, then the block closes. Then the same with
 * nothing held: no join, the block open only through the hand-off's own turns. `?seam=moved`: the
 * same, the hand-off going to another CLI. `&stop=<step>` leaves the page at that step.
 */
import { mockIPC } from "@/ipc/nativeMocks";
import { createRoot } from "react-dom/client";

import { ConversationView } from "@/app/conversation/ConversationView";
import { SidebarProvider } from "@/components/layout";
import { TooltipProvider } from "@/components/overlays";
import type { Conversation, FileEntry, ProviderEvent, Request, ThreadTurn, ThreadView, WorkerRow } from "@/ipc/generated";
import { applyActivityNews } from "@/state/activity";
import { emptyBoard, useBoard } from "@/state/board";
import { useCliThread } from "@/state/cliThread";
import { emptyThread, useApp } from "@/state/store";

const id = "held-session";
const t0 = Date.now() - 10 * 60_000;
const at = (offset: number) => t0 + offset;
const said = (itemId: string, role: "user" | "assistant", text: string, offset: number): FileEntry => ({
  event: { type: "message", itemId, role, text } as ProviderEvent,
  atMs: at(offset),
});
const turn = (userMessage: string, from: number, to: number, requests: string[], joins = false): ThreadTurn => ({
  userMessage, startedAtMs: at(from), endedAtMs: at(to), end: "done", error: null, requests, ...(joins && { joins }),
});
const worker = (wid: string, title: string, offset: number, done: boolean): WorkerRow => ({
  id: wid, number: wid === "aa11bb22" ? 1 : 2, title, agent: "claude", model: null, startedAtMs: at(offset), working: !done,
  lastEnd: done ? "done" : null, gone: false, lastEventAtMs: null, limit: null, freshContext: false, waiting: [], mcp: [],
  branch: null, base: null, diff: null, ahead: 0, merged: false,
});

const entries: FileEntry[] = [
  said("u1", "user", "Fix the two bugs", 0),
  said("a1", "assistant", "A worker is on the first.", 2_000),
  // Sent while the worker worked: its turn joins the block, and starts the second worker.
  said("u2", "user", "Also keep it fast", 60_000),
  said("a2", "assistant", "Another worker takes the second.", 62_000),
  // Sent while both work.
  said("u3", "user", "How is it going?", 120_000),
  said("a3", "assistant", "Both are still working.", 122_000),
];
const turns: ThreadTurn[] = [
  turn("u1", 0, 3_000, ["r1"]),
  turn("u2", 60_000, 63_000, ["r2"], true),
  turn("u3", 120_000, 123_000, ["r3"], true),
];
const news = (itemId: string, wid: string, offset: number): FileEntry =>
  said(itemId, "user", `[worker events wn-0123456789ab] Worker "Fix" (${wid}): finished · Done. (ev-1234abcd)`, offset);

let view: ThreadView = {
  entries,
  turns,
  workers: [worker("aa11bb22", "First bug", 1_000, false), worker("cc33dd44", "Second bug", 61_000, false)],
  // The core's work block: open from the first turn while the workers work (D163).
  block: { startedAtMs: at(100) },
};

const failing = new URLSearchParams(location.search).get("failed");
const refreshing = new URLSearchParams(location.search).get("refresh");
const seaming = new URLSearchParams(location.search).get("seam");
const stopAt = new URLSearchParams(location.search).get("stop");
const failedTurn: ThreadTurn = { ...turn("u1", 0, 3_000, ["r1"]), end: "failed", error: "The agent could not start." };
if (failing) {
  view = {
    entries: entries.slice(0, 2),
    turns: [failedTurn],
    workers: [worker("aa11bb22", "First bug", 1_000, false)],
    block: { startedAtMs: at(100) },
  };
}

const refreshed: FileEntry[] = [
  ...entries.slice(0, 2),
  // The CLI writes the compaction before the `/compact` it echoes.
  { event: { type: "compactionEnded", automatic: false, tokensBefore: 900, tokensAfter: 40, error: null } as ProviderEvent, atMs: at(20_000) },
  said("c1", "user", "/compact Besides your usual summary, keep these exactly", 10_000),
];
if (refreshing) {
  view = {
    entries: refreshed,
    turns: [turn("u1", 0, 3_000, ["r1"]), turn("c1", 10_000, 20_000, [])],
    workers: [worker("aa11bb22", "First bug", 1_000, false), worker("cc33dd44", "Second bug", 2_000, false)],
    block: { startedAtMs: at(100) },
  };
}

const fromBrigadier = (entry: FileEntry): FileEntry => ({ ...entry, said: { by: "brigadier" } });
const handedOn: FileEntry[] = [
  ...refreshed.slice(0, 3),
  fromBrigadier(said("c1", "user", "/compact Besides your usual summary, keep these exactly", 10_000)),
  fromBrigadier(said("n1", "user", "[Brigadier] A fresh session will continue this work.", 30_000)),
  said("n2", "assistant", "The hand-off note is written.", 34_000),
  // The core's view has the hand-off off a successor's first message: its word to carry on stays.
  fromBrigadier(said("s1", "user", "Carry on from the hand-off note.", 40_000)),
  said("s2", "assistant", "Picking up where the last session stopped.", 41_000),
  said("s3", "assistant", "The worker has what it needs.", 43_000),
];
const handedOnTurns = (last: Partial<ThreadTurn>): ThreadTurn[] => [
  turn("u1", 0, 3_000, ["r1"]),
  turn("c1", 10_000, 20_000, []),
  turn("n1", 30_000, 35_000, []),
  { ...turn("s1", 40_000, 44_000, [], true), ...last },
];
if (seaming) {
  view = {
    entries: handedOn,
    turns: handedOnTurns({ endedAtMs: null, end: null }),
    workers: [worker("aa11bb22", "First bug", 1_000, false), worker("cc33dd44", "Second bug", 2_000, false)],
    seams: [{ n: 1, open: false, at: "s1", carried: true, nudge: "n1", note: null, start: null, death: null, movedTo: seaming === "moved" ? "codex" : null }],
    block: { startedAtMs: at(100) },
  };
}

const conversation = {
  id, kind: "session", projectId: null, title: "Fix the two bugs", pinnedAtMs: null, createdAtMs: t0, updatedAtMs: at(123_000),
  lifecycle: "active", forkedFrom: null, sideOf: null, quotaWait: null,
  setup: {
    type: "session", repo: "/tmp/held-fixture", environment: { type: "newWorktree", branch: "brigadier/held/session", base: "main" },
    permission: "full", planMode: false, workersSeeUncommitted: null, orchestrator: { provider: "claude", model: "opus", effort: "high" },
  },
} as unknown as Conversation;

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const req = (payload as { request: Request }).request;
  switch (req.method) {
    case "threadView":
      return { method: req.method, view };
    case "getPullRequest":
      return { method: req.method, pullRequest: null };
    case "listFiles":
      return { method: req.method, files: [], truncated: false };
    default:
      return { method: req.method };
  }
});

useBoard.setState({
  board: {
    ...emptyBoard(id),
    loaded: true,
    // Filed under the latest request, as a carry-on turn names none.
    ...(seaming && { orchestratorSteps: [{ requestId: "r1", atMs: at(42_000), position: 0, kind: { type: "messaged", taskId: "aa11bb22", text: "Go on." } }] }),
  },
});
useApp.setState({
  selection: { type: "conversation", id },
  conversations: { [id]: conversation },
  threads: { [id]: emptyThread },
  summaryShown: { [id]: false },
  connection: { status: "connected", daemon: null, reason: null },
});
applyActivityNews({ conversationId: id, entries: [], ended: "done" });
useCliThread.setState((state) => ({ views: { ...state.views, [id]: view } }));

const pause = () => new Promise((resolve) => setTimeout(resolve, 300));
// A reply waits on its renderer, late on a first load: measured, it is the reply itself.
const settled = async () => {
  await pause();
  while (document.querySelector('[data-slot="reply-loading"]')) await pause();
};
const placeOf = (words: string) => [...document.querySelectorAll("main *")].findLast((each) => each.textContent?.trim() === words)?.getBoundingClientRect();
const show = (next: ThreadView) => {
  view = next;
  useCliThread.setState((state) => ({ views: { ...state.views, [id]: next } }));
};
/** The thread as drawn: each block's header, and the steered bubbles in it. */
function seen() {
  const blocks = [...document.querySelectorAll('[data-slot="aui_assistant-message-root"]')];
  return {
    blocks: blocks.length,
    headers: blocks.map((block) => block.querySelector('[data-slot="request-work-label"]')?.textContent ?? null),
    // Every line of the block that says what it waits for: the header's, the live line's.
    waits: blocks.map((block) => [...block.querySelectorAll('[data-slot="request-work-label"], [data-slot="thread-status"], [data-slot="group-live-line"]')]
      .map((line) => line.textContent ?? "").filter((text) => text.startsWith("Waiting"))),
    errors: blocks.map((block) => !!block.querySelector('[data-slot="error-state"]')),
    // What the thread said after the error, in time order: above is only what came before it.
    afterError: blocks.map((block) => {
      const error = block.querySelector('[data-slot="error-state"]');
      return error ? [...block.querySelectorAll('[data-slot="aui_assistant-message-content"]')].filter((text) => !!(error.compareDocumentPosition(text) & Node.DOCUMENT_POSITION_FOLLOWING)).length : null;
    }),
    buttons: blocks.map((block) => [...block.querySelectorAll('[data-slot="error-state"] button:not([data-slot="error-state-details"])')].map((button) => button.textContent ?? "")),
    steers: blocks.map((block) => [...block.querySelectorAll('[data-slot="request-steer"]')].map((steer) => steer.textContent?.replace(/\s*(?:Yesterday|\d).*$/, "").trim() ?? "")),
    // The gap over each block's live line, from what shows above it (demo12: one gap everywhere).
    gaps: blocks.flatMap((block) => {
      const line = block.querySelector('[data-slot="thread-status"]');
      let above = line?.closest('[data-slot="trailing-line"]')?.previousElementSibling ?? null;
      while (above && above.getBoundingClientRect().height === 0) above = above.previousElementSibling;
      return line && above ? [Math.round(line.getBoundingClientRect().top - above.getBoundingClientRect().bottom)] : [];
    }),
    // Every line that says the block is live, and what the composer says of it.
    liveLines: blocks.map((block) => block.querySelectorAll('[data-slot="thread-status"]').length),
    composer: [...document.querySelectorAll("main *")].find((each) => !each.closest('[data-slot="aui_assistant-message-root"]') && each.children.length === 0 && each.textContent?.startsWith("Working for"))?.textContent ?? null,
    // What the block shows in order, top to bottom: its dividers, its replies, its row of a message to a worker.
    order: blocks.map((block) => [...block.querySelectorAll('[data-slot="compaction"], [data-slot="seam"], [data-slot="orchestrator-step"], [data-slot="aui_assistant-message-content"]')]
      .map((each) => (each.getAttribute("data-slot") === "seam" ? "moved" : each.getAttribute("data-slot") === "compaction" ? "divider" : (each.textContent ?? "").trim().replace(/\s+/g, " ").slice(0, 40)))),
    // A refresh's divider: how far under the reply before it, and whether the live line is under it.
    refresh: (() => {
      const divider = document.querySelector('[data-slot="compaction"]')?.getBoundingClientRect();
      const reply = placeOf("A worker is on the first.");
      const line = document.querySelector('[data-slot="thread-status"]')?.getBoundingClientRect();
      const lower = placeOf("The first bug is fixed.");
      return {
        gap: divider && reply ? Math.round(divider.top - reply.bottom) : null,
        lineUnder: divider && line ? line.top >= divider.bottom : null,
        wokenUnder: divider && lower ? lower.top >= divider.bottom : null,
      };
    })(),
  };
}

/** The conversation in the app's own layout (App.tsx), without its sidebar and dialogs. */
function Page() {
  return (
    <TooltipProvider>
      <SidebarProvider className="flex h-screen flex-col" defaultOpen={false}>
        <main className="relative flex h-full min-w-0 flex-1">
          <div className="relative flex h-full min-w-0 flex-1 flex-col">
            <ConversationView selection={{ type: "conversation", id }} />
          </div>
        </main>
      </SidebarProvider>
    </TooltipProvider>
  );
}

/** A failed turn's block, step by step: its worker works, then its end wakes the thread or nothing follows. */
async function driveFailed() {
  await pause();
  const works = seen();
  const worked = [worker("aa11bb22", "First bug", 1_000, true)];
  const closed = { startedAtMs: at(100), endedAtMs: at(202_000) };
  const steps: Record<string, unknown> = { works };
  if (failing === "wake") {
    const woke = [...entries.slice(0, 2), news("n1", "aa11bb22", 200_000)];
    show({ ...view, entries: woke, turns: [failedTurn, { ...turn("n1", 200_000, 0, ["r1"]), endedAtMs: null, end: null }], workers: worked });
    await pause();
    steps.woken = seen();
    show({
      ...view,
      entries: [...woke, said("a4", "assistant", "The first bug is fixed.", 201_000)],
      turns: [failedTurn, turn("n1", 200_000, 202_000, ["r1"])],
      block: closed,
    });
  } else {
    show({ ...view, workers: worked, block: closed });
  }
  await pause();
  steps.closed = seen();
  const result = document.createElement("pre");
  result.id = "held-block-result";
  result.textContent = JSON.stringify(steps);
  document.body.append(result);
}

/** A block held through a refresh, step by step: its worker works, then its end wakes the thread or nothing follows. */
async function driveRefresh() {
  await settled();
  const held = seen();
  const worked = [worker("aa11bb22", "First bug", 1_000, true), worker("cc33dd44", "Second bug", 2_000, true)];
  const steps: Record<string, unknown> = { held };
  if (refreshing === "wake") {
    const woke = [...refreshed, said("a4", "assistant", "The first bug is fixed.", 201_000)];
    const before = view.turns[0]!;
    const oneWorks = [worker("aa11bb22", "First bug", 1_000, true), worker("cc33dd44", "Second bug", 2_000, false)];
    // The first worker's end wakes it, under the `/compact`.
    show({ ...view, entries: woke, turns: [before, { ...turn("c1", 10_000, 0, []), endedAtMs: null, end: null }], workers: oneWorks });
    await settled();
    steps.woken = seen();
    // That turn ends with the second still working.
    const over = turn("c1", 10_000, 202_000, []);
    show({ ...view, turns: [before, over] });
    await settled();
    steps.over = seen();
    // The user writes meanwhile: their message joins the block, its own work under it.
    show({
      ...view,
      entries: [...woke, said("u9", "user", "How is it going?", 250_000), said("a9", "assistant", "One still works.", 251_000)],
      turns: [before, over, turn("u9", 250_000, 252_000, ["r9"], true)],
    });
    await settled();
    steps.asked = seen();
  }
  show({ ...view, workers: worked, block: { startedAtMs: at(100), endedAtMs: at(300_000) } });
  await settled();
  steps.closed = seen();
  const result = document.createElement("pre");
  result.id = "held-block-result";
  result.textContent = JSON.stringify(steps);
  document.body.append(result);
}

const turnNews = (word: { opened?: boolean; ended?: "done" }) => applyActivityNews({ conversationId: id, entries: [], ...word } as Parameters<typeof applyActivityNews>[0]);

/** A block held through a hand-off, step by step: the successor's turn runs, ends, and the block closes. */
async function driveSeam() {
  const steps: Record<string, unknown> = {};
  const step = async (name: string, next?: ThreadView) => {
    if (next) show(next);
    await settled();
    steps[name] = seen();
    return name === stopAt;
  };
  const worked = [worker("aa11bb22", "First bug", 1_000, true), worker("cc33dd44", "Second bug", 2_000, true)];
  const closed = { startedAtMs: at(100), endedAtMs: at(300_000) };
  // The tab and composer's word that it works, for the steps its turn runs.
  turnNews({ opened: true });
  const running = await step("running");
  if (!running) turnNews({ ended: "done" });
  const done =
    running ||
    (await step("over", { ...view, turns: handedOnTurns({}) })) ||
    (await step("closed", { ...view, workers: worked, block: closed })) ||
    (await step("nothingHeld", { ...view, turns: handedOnTurns({ endedAtMs: null, end: null, joins: false }), block: { startedAtMs: at(100) } }));
  void done;
  const result = document.createElement("pre");
  result.id = "held-block-result";
  result.textContent = JSON.stringify(steps);
  document.body.append(result);
}

async function drive() {
  await pause();
  const bothWork = seen();
  // The first worker finishes; its word joins the block.
  show({
    ...view,
    entries: [...entries, news("n1", "aa11bb22", 200_000), said("a4", "assistant", "The first bug is fixed.", 201_000)],
    turns: [...turns, turn("n1", 200_000, 202_000, [])],
    workers: [worker("aa11bb22", "First bug", 1_000, true), worker("cc33dd44", "Second bug", 61_000, false)],
  });
  await pause();
  const oneWorks = seen();
  // The second finishes too: the block's work is over, and folds under its answer.
  show({
    ...view,
    entries: [...view.entries, news("n2", "cc33dd44", 300_000), said("a5", "assistant", "Both bugs are fixed.", 301_000)],
    turns: [...view.turns, turn("n2", 300_000, 302_000, [])],
    workers: [worker("aa11bb22", "First bug", 1_000, true), worker("cc33dd44", "Second bug", 61_000, true)],
    block: { startedAtMs: at(100), endedAtMs: at(302_000) },
  });
  await pause();
  const done = seen();
  const result = document.createElement("pre");
  result.id = "held-block-result";
  result.textContent = JSON.stringify({ bothWork, oneWorks, done });
  document.body.append(result);
}

createRoot(document.getElementById("root")!).render(<Page />);
void (seaming ? driveSeam() : refreshing ? driveRefresh() : failing ? driveFailed() : drive());
