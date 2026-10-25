/**
 * A standalone Vite entry, excluded from the app entry and production build: the whole app
 * (App.tsx) open on a long project session whose turn streams, the way the daemon's
 * `threadNews` says it (a delta at a time), with other sessions working in the sidebar, then the
 * same with an approval waiting. IPC is answered here, no daemon.
 *
 * `?drive=1` runs the phases and writes, per phase, the frames and their gaps, the long tasks,
 * React's commits (with a profiling build, see scripts/measure-stream-load.mjs) and how often
 * chosen components drew, to `<pre id="stream-load-result">`. `turns=<n>` sets the thread's
 * length, `rate=<n>` the deltas a second, `phase=<ms>` each phase's length; `count=0` counts no
 * draws (counting costs time of its own). `extra=1` goes on: typing in the composer while it streams
 * (the measuring script types, as `data-typing` asks), opening a second long session and
 * switching between the two, jumping to the first turn from the rail, finding in the thread, and
 * scrolling the whole thread.
 */
// First: React DOM finds the counting hook as it loads.
import { countRenders, renderCounts } from "@/fixtures/renderCounts";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { Profiler, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { App } from "@/app/App";
import { openConversation, setPinnedSummary } from "@/state/actions";
import type { Conversation, FileEntry, ProviderEvent, Project, Request, ThreadTurn, ThreadView } from "@/ipc/generated";
import { fromSnapshot, useActivity } from "@/state/activity";
import { emptyBoard, type ShownApproval, useBoard } from "@/state/board";
import { applyThreadNews, useCliThread } from "@/state/cliThread";
import { useSessionTabs } from "@/state/sessionTabs";
import { emptyThread, useApp } from "@/state/store";

const query = new URLSearchParams(location.search);
document.documentElement.dataset.platform = "macos";
localStorage.removeItem("brigadier.sidebarWidth");

const ID = "stream-load";
const SECOND = "stream-load-2";
const TURNS = Number(query.get("turns") ?? 150);
const RATE = Number(query.get("rate") ?? 50);
const PHASE_MS = Number(query.get("phase") ?? 4000);
const OTHERS = 6;
const start = Date.now() - 86_400_000;

const words = "The window stays put while what is inside it scrolls and only the pane under the pointer moves".split(" ");
const sentence = (seed: number, length: number) => Array.from({ length }, (_, index) => words[(seed + index * 7) % words.length]).join(" ");
const output = (turn: number, call: number) => Array.from({ length: 20 }, (_, line) => `line ${line + 1} of call ${call} in turn ${turn}: ${sentence(line, 8)}`).join("\n");

/** A long session as its CLI keeps it: per turn a question, thinking, six commands and an answer. */
function longView(asked = "Question"): ThreadView {
  const entries: FileEntry[] = [];
  const turns: ThreadTurn[] = [];
  for (let turn = 0; turn < TURNS; turn++) {
    const at = start + turn * 60_000;
    const user = `u${turn}`;
    entries.push({ event: { type: "message", itemId: user, role: "user", text: `${asked} ${turn + 1}: ${sentence(turn, 12)}` }, atMs: at });
    entries.push({ event: { type: "reasoning", itemId: `t${turn}`, text: sentence(turn, 40) }, atMs: at + 1000, updatedAtMs: at + 3000 });
    for (let call = 0; call < 6; call++) {
      entries.push({
        event: { type: "command", itemId: `c${turn}-${call}`, command: `pnpm test --filter part-${call}`, cwd: "/tmp/stream-load", status: "completed", exitCode: 0, output: output(turn, call), durationMs: 1200 } as ProviderEvent,
        atMs: at + 4000 + call * 2000,
        updatedAtMs: at + 5000 + call * 2000,
      });
    }
    const last = turn === TURNS - 1;
    if (!last) {
      entries.push({ event: { type: "message", itemId: `a${turn}`, role: "assistant", text: Array.from({ length: 4 }, (_, index) => `Paragraph ${index + 1}. ${sentence(turn + index, 30)}.`).join("\n\n") }, atMs: at + 20_000 });
    }
    turns.push({ userMessage: user, startedAtMs: at, endedAtMs: last ? null : at + 30_000, end: last ? null : "done", error: null, requests: [] });
  }
  return { entries, turns };
}

const conversation = (id: string, title: string) => ({
  id, kind: "session", projectId: "stream-project", title,
  pinnedAtMs: null, createdAtMs: start, updatedAtMs: start, lifecycle: "active",
  forkedFrom: null, sideOf: null, quotaWait: null,
  setup: {
    type: "session", repo: "/tmp/stream-load",
    environment: { type: "local" },
    permission: "ask", planMode: false, workersSeeUncommitted: null,
    orchestrator: { provider: "claude", model: "opus", effort: "high" },
  },
}) as unknown as Conversation;

const others = Array.from({ length: OTHERS }, (_, index) => conversation(`other-${index}`, `Other session ${index + 1}`));
const view = longView();
// The second long session's view is read when it first opens.
const secondView = longView("Second question");
useCliThread.setState({ views: { [ID]: view } });
useBoard.setState({ board: { ...emptyBoard(ID), loaded: true, run: "running" } });
useApp.setState((state) => ({
  info: { platform: "macos", version: "fixture", processStartMs: 0, smoke: false, firstLaunch: false, budgetTolerance: 1 },
  connection: { status: "connected", daemon: null, reason: null },
  settings: { ...state.settings, onboarded: true, introSeen: true },
  projects: { "stream-project": { id: "stream-project", name: "stream-load", createdAtMs: start, repos: [{ path: "/tmp/stream-load", name: "stream-load" }], prefs: {}, trust: [] } as unknown as Project },
  conversations: Object.fromEntries([conversation(ID, "A long working session"), conversation(SECOND, "Another long session"), ...others].map((each) => [each.id, each])),
  threads: { [ID]: { ...emptyThread } },
  selection: { type: "conversation", id: ID },
}));
useActivity.setState({
  byConversation: Object.fromEntries(
    [ID, SECOND, ...others.map((each) => each.id)].map((id) => [id, fromSnapshot({ conversationId: id, run: "running", turnOpen: true, tasks: [], approvals: [], questions: [] })]),
  ),
});
useSessionTabs.setState({ sessions: {} });

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const request = (payload as { request: Request }).request;
  switch (request.method) {
    case "threadView": return { method: "threadView", view: request.conversationId === SECOND ? secondView : (useCliThread.getState().views[ID] ?? view) };
    case "listMessages": return { method: "listMessages", page: { items: [], hasMore: false } };
    case "getConversation": return { method: "getConversation", view: conversationView(request.id) };
    default: return { method: request.method };
  }
}, { shouldMockEvents: true });
mockWindows("main");

/** What the daemon says of a session's chat when it opens: nothing beyond its CLI thread. */
function conversationView(id: string) {
  return {
    conversation: useApp.getState().conversations[id], context: null, messages: { messages: [], hasMore: false },
    tasks: [], approvals: [], questions: [], requests: [], workerSteps: [], orchestratorSteps: [], machineSteps: [], thinking: [], compactions: [], decisions: [],
    queue: { items: [], paused: false }, run: "running", runRequest: null, head: null, ratings: {}, streaming: null, notices: [], previews: [], backgroundCleared: [], mcpServers: [], peerMessages: [],
  };
}

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/** What a phase measures. */
const meter = {
  commits: 0,
  commitMs: 0,
  longTasks: [] as number[],
  gaps: [] as number[],
};
new PerformanceObserver((list) => {
  for (const entry of list.getEntries()) meter.longTasks.push(entry.duration);
}).observe({ type: "longtask", buffered: false });
let last = 0;
const frames = (time: number) => {
  if (last) meter.gaps.push(time - last);
  last = time;
  requestAnimationFrame(frames);
};
requestAnimationFrame(frames);
/** The components whose draws each phase counts. */
const COUNTED = ["ConversationView", "RequestBlock", "ConversationComposer", "ApprovalSlot", "ApprovalAction", "SessionTabBar", "PinnedSummary", "MessageRail", "AppSidebar"];

function reset() {
  meter.commits = 0;
  meter.commitMs = 0;
  // Counting walks the tree at each commit: `count=0` leaves it out where the time is measured.
  if (query.get("count") !== "0") countRenders(COUNTED);
  meter.longTasks = [];
  meter.gaps = [];
}

function summary(ms: number) {
  const gaps = meter.gaps.toSorted((a, b) => a - b);
  const at = (q: number) => Math.round(gaps[Math.min(gaps.length - 1, Math.floor(gaps.length * q))] ?? 0);
  return {
    frames: gaps.length,
    fps: Math.round((gaps.length / ms) * 1000),
    gapP50: at(0.5),
    gapP95: at(0.95),
    gapMax: Math.round(gaps.at(-1) ?? 0),
    over50: gaps.filter((gap) => gap > 50).length,
    longTasks: meter.longTasks.length,
    longTaskMs: Math.round(meter.longTasks.reduce((sum, each) => sum + each, 0)),
    longTaskMax: Math.round(Math.max(0, ...meter.longTasks)),
    commits: meter.commits,
    commitMs: Math.round(meter.commitMs),
    renders: renderCounts(),
  };
}

let deltas = 0;
/** The open turn's answer, said a few words at a time, a command's output every few deltas. */
async function stream(ms: number) {
  const until = performance.now() + ms;
  const every = 1000 / RATE;
  let call = 0;
  let next = performance.now();
  while (performance.now() < until) {
    deltas += 1;
    const item = `live-${Math.floor(deltas / 200)}`;
    // A paragraph ends every dozen deltas: a reply shows a finished line at a time (D9 b).
    const end = deltas % 12 === 0 ? "\n\n" : " ";
    applyThreadNews({ conversationId: ID, entries: [{ event: { type: "messageDelta", itemId: item, text: `${sentence(deltas, 3)}${end}` }, atMs: Date.now() }] });
    // The other sessions stream too: news the app has no view of.
    applyThreadNews({ conversationId: `other-${deltas % OTHERS}`, entries: [{ event: { type: "messageDelta", itemId: `o${deltas}`, text: "more " }, atMs: Date.now() }] });
    if (deltas % 10 === 0) {
      call += 1;
      applyThreadNews({ conversationId: ID, entries: [{ event: { type: "commandOutputDelta", itemId: `live-c${Math.floor(call / 20)}`, text: `output line ${call}\n` }, atMs: Date.now() }] });
    }
    next += every;
    await wait(Math.max(0, next - performance.now()));
  }
}

function addApproval() {
  const approval: ShownApproval = {
    id: "ap-1", conversationId: ID, taskId: null, requestId: null, position: 0, state: { type: "pending" }, createdAtMs: Date.now(), resolvedAtMs: null,
    subject: { type: "cli", request: { id: "ap-1", kind: "command", tool: "Bash", command: "git push origin main", cwd: null, paths: [], reason: null, escalation: false, input: null, grant: "git push" } },
  };
  useBoard.setState((state) => ({ board: state.board && { ...state.board, approvals: { ...state.board.approvals, [approval.id]: approval } } }));
  useActivity.setState((state) => ({
    byConversation: { ...state.byConversation, [ID]: fromSnapshot({ conversationId: ID, run: "running", turnOpen: true, tasks: [], approvals: ["ap-1"], questions: [] }) },
  }));
}

async function drive() {
  // The live turn's first words, and the thread settled.
  applyThreadNews({ conversationId: ID, entries: [{ event: { type: "command", itemId: "live-c0", command: "pnpm build", cwd: null, status: "inProgress", exitCode: null, output: "", durationMs: null } as ProviderEvent, atMs: Date.now() }] });
  for (let tries = 0; tries < 100 && !document.querySelector('[data-slot="aui_thread-viewport"]'); tries++) await wait(50);
  await wait(1500);
  const seen: Record<string, unknown> = { turns: TURNS, entries: view.entries.length, rate: RATE };
  reset();
  await wait(PHASE_MS);
  seen.idle = summary(PHASE_MS);
  // The measuring script's CPU profile starts here.
  document.documentElement.dataset.streaming = "";
  reset();
  await stream(PHASE_MS);
  seen.stream = { ...summary(PHASE_MS), deltas };
  addApproval();
  await wait(300);
  seen.approvalShown = Boolean(document.querySelector('[data-slot="approval-slot"]'));
  reset();
  await stream(PHASE_MS);
  seen.streamApproval = { ...summary(PHASE_MS), deltas };
  if (query.get("extra") === "1") await extra(seen);
  return seen;
}

/** Resolves once the frame after this one has painted. */
const painted = () => new Promise<void>((resolve) => requestAnimationFrame(() => setTimeout(resolve, 0)));

/** Milliseconds from `action` to the first painted frame where `ready` holds. */
async function timeTo(action: () => unknown, ready: () => boolean, most = 600): Promise<number> {
  const asked = performance.now();
  await action();
  for (let tries = 0; tries < most; tries++) {
    await painted();
    const now = performance.now();
    if (ready()) return Math.round(now - asked);
  }
  throw new Error(`it never got there (${document.documentElement.dataset.phase} ${document.documentElement.dataset.step ?? ""})`);
}

const viewport = () => document.querySelector<HTMLElement>('[data-slot="aui_thread-viewport"]');
/** Whether the open thread shows its newest turn, asked as `asked`. */
const showing = (asked: string) => () => (viewport()?.textContent ?? "").includes(`${asked} ${TURNS}:`);

/** Whether a row of the open thread holding `text` is mounted and shows in the view. */
const inView = (text: string) => () => {
  const sides = viewport()?.getBoundingClientRect();
  if (!sides) return false;
  return [...document.querySelectorAll<HTMLElement>('[data-slot="aui_message-group"] > [data-thread-row]')].some((row) => {
    const box = row.getBoundingClientRect();
    return box.bottom > sides.top && box.top < sides.bottom && (row.textContent ?? "").includes(text);
  });
};

/** What Find's bar counts. */
const counted = () => (document.querySelector('[data-slot="thread-find-count"]')?.textContent ?? "").trim();

/** `text` typed into a field React holds, as one change. */
function typeIn(field: HTMLInputElement, text: string) {
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set?.call(field, text);
  field.dispatchEvent(new Event("input", { bubbles: true }));
}

/** Typing, switching sessions, jumping to the first turn, finding and scrolling, while it streams. */
async function extra(seen: Record<string, unknown>) {
  // The approval answered: the composer is back in its place.
  useBoard.setState((state) => ({ board: state.board && { ...state.board, approvals: {} } }));
  useActivity.setState((state) => ({
    byConversation: { ...state.byConversation, [ID]: fromSnapshot({ conversationId: ID, run: "running", turnOpen: true, tasks: [], approvals: [], questions: [] }) },
  }));
  await wait(300);
  const streaming = stream(PHASE_MS * 4);
  // Typing: from each key to the frame that shows it.
  const editor = document.querySelector<HTMLElement>('[data-lexical-editor="true"]');
  if (!editor) throw new Error("no composer");
  editor.focus();
  const keys: number[] = [];
  const onKey = (event: KeyboardEvent) => {
    const at = event.timeStamp;
    void painted().then(() => keys.push(performance.now() - at));
  };
  window.addEventListener("keydown", onKey, true);
  // The browser's own count, key to painted frame (Event Timing): only those of 16 ms or more.
  const slow: number[] = [];
  const timing = new PerformanceObserver((list) => {
    for (const entry of list.getEntries()) if (entry.name === "keydown") slow.push(entry.duration);
  });
  timing.observe({ type: "event", durationThreshold: 16 } as PerformanceObserverInit);
  reset();
  document.documentElement.dataset.phase = "typing";
  document.documentElement.dataset.typing = "";
  while (!("typed" in document.documentElement.dataset)) await wait(50);
  await wait(200);
  window.removeEventListener("keydown", onKey, true);
  timing.disconnect();
  const sorted = keys.toSorted((a, b) => a - b);
  seen.typing = {
    ...summary(PHASE_MS), keys: keys.length,
    keyP50: Math.round(sorted[Math.floor(sorted.length / 2)] ?? 0), keyP95: Math.round(sorted[Math.floor(sorted.length * 0.95)] ?? 0), keyMax: Math.round(sorted.at(-1) ?? 0),
    typed: editor.textContent?.length ?? 0,
    keysOver16: slow.length, slowestKey: Math.round(Math.max(0, ...slow)),
  };
  // Switching: the second session's first opening reads its view; after that both are held.
  reset();
  document.documentElement.dataset.phase = "switching";
  const switches: Record<string, number> = {};
  switches.openSecond = await timeTo(() => openConversation(SECOND), showing("Second question"));
  for (let round = 1; round <= 3; round++) {
    switches[`back${round}`] = await timeTo(() => openConversation(ID), showing("Question"));
    switches[`again${round}`] = await timeTo(() => openConversation(SECOND), showing("Second question"));
  }
  switches.back = await timeTo(() => openConversation(ID), showing("Question"));
  seen.switching = { ...switches, ...summary(1) };
  // A jump from the rail to the first turn, far out of what is mounted.
  await wait(500);
  reset();
  document.documentElement.dataset.phase = "jump";
  // The rail's first marker: the oldest turn the thread holds (a long one holds its newest turns).
  // The pinned summary leaves the rail no room beside the column at this window's width: unpinned.
  setPinnedSummary(false);
  for (let tries = 0; tries < 40 && !document.querySelector('[data-slot="message-rail"] button'); tries++) await wait(50);
  const first = document.querySelector<HTMLElement>('[data-slot="message-rail"] button');
  const label = first?.getAttribute("aria-label")?.split(":")[0] ?? "";
  const jump = first && label ? await timeTo(() => first.click(), inView(`${label}:`)) : null;
  seen.jump = { ms: jump, to: label, rail: document.querySelectorAll('[data-slot="message-rail"] button').length, ...summary(1) };
  // Find: a word in one old turn, gone to; then a word in every turn, counted over the whole thread.
  await wait(500);
  reset();
  try {
    document.documentElement.dataset.phase = "find";
    window.dispatchEvent(new KeyboardEvent("keydown", { code: "KeyF", key: "f", metaKey: true, bubbles: true }));
    await painted();
    const bar = document.querySelector<HTMLInputElement>('[data-slot="thread-find"] input');
    if (!bar) throw new Error("no find bar");
    document.documentElement.dataset.step = "count one";
    const one = await timeTo(() => typeIn(bar, "Question 2:"), () => /[1-9]/.test(counted()), 3600);
    document.documentElement.dataset.step = "go to it";
    const goneTo = await timeTo(() => bar.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true })), inView("Question 2:"));
    const everyFrom = performance.now();
    typeIn(bar, "Paragraph 1.");
    let shown = "";
    let stableFrom = performance.now();
    while (performance.now() - everyFrom < PHASE_MS * 3 && performance.now() - stableFrom < 1000) {
      await wait(50);
      const now = counted();
      if (now !== shown) {
        shown = now;
        stableFrom = performance.now();
      }
    }
    const every = Math.round(stableFrom - everyFrom);
    bar.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    seen.find = { one, goneTo, every, label: shown, ...summary(performance.now() - everyFrom) };
  } catch (error) {
    seen.find = { error: String(error) };
  }
  // Scrolling the thread from its bottom up towards its top, a screen's worth each frame.
  const scroller = viewport();
  if (!scroller) throw new Error("no thread");
  scroller.scrollTop = scroller.scrollHeight;
  await wait(1000);
  reset();
  document.documentElement.dataset.phase = "scroll";
  const scrolledFrom = performance.now();
  let steps = 0;
  while (scroller.scrollTop > 0 && performance.now() - scrolledFrom < PHASE_MS) {
    scroller.scrollTop -= 400;
    steps += 1;
    await new Promise((resolve) => requestAnimationFrame(resolve));
  }
  const scrolledMs = performance.now() - scrolledFrom;
  seen.scroll = { ...summary(scrolledMs), steps, height: scroller.scrollHeight, reachedTop: scroller.scrollTop === 0 };
  await streaming;
}

function Fixture() {
  const [result, setResult] = useState("");
  useEffect(() => {
    if (query.get("drive") !== "1") return;
    void drive()
      .then((seen) => setResult(JSON.stringify(seen)))
      .catch((error: unknown) => setResult(JSON.stringify({ error: String(error instanceof Error ? error.stack : error) })));
  }, []);
  return (
    <>
      <Profiler
        id="app"
        onRender={(_id, _phase, actual) => {
          meter.commits += 1;
          meter.commitMs += actual;
        }}
      >
        <App />
      </Profiler>
      {result && <pre id="stream-load-result" hidden>{result}</pre>}
    </>
  );
}

createRoot(document.getElementById("root")!).render(<Fixture />);
