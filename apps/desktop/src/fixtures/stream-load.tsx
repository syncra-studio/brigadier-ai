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
 * draws (counting costs time of its own).
 */
// First: React DOM finds the counting hook as it loads.
import { countRenders, renderCounts } from "@/fixtures/renderCounts";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { Profiler, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { App } from "@/app/App";
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
const TURNS = Number(query.get("turns") ?? 150);
const RATE = Number(query.get("rate") ?? 50);
const PHASE_MS = Number(query.get("phase") ?? 4000);
const OTHERS = 6;
const start = Date.now() - 86_400_000;

const words = "The window stays put while what is inside it scrolls and only the pane under the pointer moves".split(" ");
const sentence = (seed: number, length: number) => Array.from({ length }, (_, index) => words[(seed + index * 7) % words.length]).join(" ");
const output = (turn: number, call: number) => Array.from({ length: 20 }, (_, line) => `line ${line + 1} of call ${call} in turn ${turn}: ${sentence(line, 8)}`).join("\n");

/** A long session as its CLI keeps it: per turn a question, thinking, six commands and an answer. */
function longView(): ThreadView {
  const entries: FileEntry[] = [];
  const turns: ThreadTurn[] = [];
  for (let turn = 0; turn < TURNS; turn++) {
    const at = start + turn * 60_000;
    const user = `u${turn}`;
    entries.push({ event: { type: "message", itemId: user, role: "user", text: `Question ${turn + 1}: ${sentence(turn, 12)}` }, atMs: at });
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
useCliThread.setState({ views: { [ID]: view } });
useBoard.setState({ board: { ...emptyBoard(ID), loaded: true, run: "running" } });
useApp.setState((state) => ({
  info: { platform: "macos", version: "fixture", processStartMs: 0, smoke: false, firstLaunch: false, budgetTolerance: 1 },
  connection: { status: "connected", daemon: null, reason: null },
  settings: { ...state.settings, onboarded: true, introSeen: true },
  projects: { "stream-project": { id: "stream-project", name: "stream-load", createdAtMs: start, repos: [{ path: "/tmp/stream-load", name: "stream-load" }], prefs: {}, trust: [] } as unknown as Project },
  conversations: Object.fromEntries([conversation(ID, "A long working session"), ...others].map((each) => [each.id, each])),
  threads: { [ID]: { ...emptyThread } },
  selection: { type: "conversation", id: ID },
}));
useActivity.setState({
  byConversation: Object.fromEntries(
    [ID, ...others.map((each) => each.id)].map((id) => [id, fromSnapshot({ conversationId: id, run: "running", turnOpen: true, tasks: [], approvals: [], questions: [] })]),
  ),
});
useSessionTabs.setState({ sessions: {} });

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const request = (payload as { request: Request }).request;
  switch (request.method) {
    case "threadView": return { method: "threadView", view: useCliThread.getState().views[ID] ?? view };
    case "listMessages": return { method: "listMessages", page: { items: [], hasMore: false } };
    case "getConversation": return new Promise(() => {});
    default: return { method: request.method };
  }
}, { shouldMockEvents: true });
mockWindows("main");

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
    applyThreadNews({ conversationId: ID, entries: [{ event: { type: "messageDelta", itemId: item, text: `${sentence(deltas, 3)} ` }, atMs: Date.now() }] });
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
  return seen;
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
