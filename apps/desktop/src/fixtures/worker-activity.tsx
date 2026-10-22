/** Tests lifecycle rows with no commands, internal ids or diff totals, and the Workers strip. */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { TeamSentence } from "@/app/conversation/activity/TeamSentence";
import { useWorkersStrip, WorkersStrip } from "@/app/conversation/activity/WorkersStrip";
import { WorkersSummary } from "@/app/side-panel/WorkerSummary";
import { AgentsPanelContext } from "@/app/conversation/WorkerChip";
import { WorkersTab } from "@/app/side-panel/Agents";
import { TooltipProvider } from "@/components/overlays";
import night from "@/fixtures/boards/overnight-2026-10-03.json";
import type { Task } from "@/ipc/generated";
import { createStore } from "zustand/vanilla";

import { BoardStoreContext, emptyBoard, useBoard } from "@/state/board";
import { rowOf, showRows, threadOf } from "@/fixtures/workerRows";
const base = Object.values(night.tasks)[0] as unknown as Task;
// One request, not answered yet: its finished worker stays in the strip.
const conversationId = "worker-fixture";
const task = (id: string, number: number, state: Task["state"], blockedReason: string | null = null): Task => ({
  ...base, id, number, title: `Check ${id}`, conversationId, state, blockedReason, role: null,
  attempts: [], requestId: "r1", gateLink: null, candidate: null, report: null, quotaWait: null,
  createdAtMs: 8000, updatedAtMs: 80000,
});
const tasks = { active: task("active", 1, "running"), blocked: task("blocked", 2, "blocked", "Waiting for plan review"),
  queued: task("queued", 3, "queued"), completed: task("completed", 4, "done"),
  // Stopped before it landed: it counts as stopped, never as done (BUILD-PLAN A7).
  stopped: task("stopped", 5, "stopped") };
useBoard.setState({ board: { ...emptyBoard(conversationId), loaded: true, tasks } });
// The same workers as their rows (S14), newest first: one at work, one waiting on the user, one
// held by a usage limit, one done and one stopped.
showRows(conversationId, [
  rowOf(tasks.queued, { working: true, limit: { kind: "usageWindow", resetsAtMs: 3_600_000, window: "five_hour" } }),
  rowOf(tasks.blocked, { working: true, waiting: [{ kind: "permission", what: "pnpm install", card: null }] }),
  rowOf(tasks.active),
  rowOf(tasks.completed, { lastEventAtMs: tasks.completed.updatedAtMs }),
  rowOf(tasks.stopped, { lastEventAtMs: tasks.stopped.updatedAtMs }),
]);
// A session whose workers all finished: no strip.
const idleId = "worker-fixture-idle";
const idleBoard = createStore(() => ({
  board: { ...emptyBoard(idleId), loaded: true, tasks: { done: { ...tasks.completed, conversationId: idleId }, stopped: { ...tasks.stopped, conversationId: idleId } } },
  orchestrator: null,
}));
// What the page asked the daemon (a worker's Stop sends `stopTask`).
const sent: string[] = [];
mockIPC((cmd, args) => {
  sent.push(JSON.stringify({ cmd, args }));
  const method = (args as { request?: { method?: string } } | undefined)?.request?.method;
  if (method === "workerThread") return { method, view: threadOf([{ streamSeq: 1, atMs: 9000, event: { type: "message", itemId: "brief", role: "user", text: "Check the blocked case." } }]) };
  return null;
});
function Fixture() {
  const [result, setResult] = useState("");
  const [panel, setPanel] = useState<string | null | undefined>();
  useEffect(() => {
    // A waiting worker's Stop on its Workers list row, with nothing opened first (#201).
    const listStop = setTimeout(() => document.querySelector<HTMLButtonElement>('[data-slot="worker-list"] button[aria-label="Stop Check blocked"]')?.click(), 50);
    // Open the strip, as a click on its count does.
    const open = setTimeout(() => document.querySelector<HTMLButtonElement>('[data-slot="workers-strip"] button[aria-expanded]')?.click(), 100);
    // The running worker's own Stop, on its strip row (UI audit M4).
    const stop = setTimeout(() => document.querySelector<HTMLButtonElement>('[data-slot="workers-strip"] button[aria-label="Stop Check active"]')?.click(), 250);
    let seen: Record<string, unknown> = {};
    const snapshot = setTimeout(() => { seen = {
      thread: Object.fromEntries([...document.querySelectorAll('main > section[data-task]')].map((row) => [row.getAttribute("data-task"), row.textContent])),
      summary: document.querySelector('[aria-label="Session workers"]')?.textContent,
      list: document.querySelector('[data-slot="worker-list"]')?.textContent,
      strip: document.querySelector('[data-slot="workers-strip"]')?.textContent,
      noStrip: document.querySelector('[data-slot="idle-strip"]')?.childElementCount === 0,
      stripRows: document.querySelectorAll('[data-slot="workers-strip-row"]').length,
      stripStops: [...document.querySelectorAll<HTMLButtonElement>('[data-slot="workers-strip"] [data-slot="worker-stop"]')].map((button) => button.getAttribute("aria-label")),
      listStops: [...document.querySelectorAll<HTMLButtonElement>('[data-slot="worker-list"] [data-slot="worker-stop"]')].map((button) => button.getAttribute("aria-label")),
      headerStop: document.querySelector('[data-slot="worker-view"] [data-slot="worker-header"] [data-slot="worker-stop"]')?.getAttribute("aria-label"),
      doneHeaderStop: !!document.querySelector('[data-slot="worker-view-done"] [data-slot="worker-stop"]'),
      sent: sent.filter((call) => call.includes("stopTask")),
      avatars: [...document.querySelectorAll('[data-slot="task-row"] span[aria-hidden] svg')].map((svg) => svg.getBoundingClientRect().width),
    }; }, 350);
    // A worker opened from the rail: its thread, read only, with no composer (G19.Q6).
    const rail = setTimeout(() => [...document.querySelectorAll<HTMLButtonElement>('[data-slot="workers-strip-row"]')].find((row) => row.textContent?.includes("Check blocked"))?.click(), 380);
    const timer = setTimeout(() => {
      const view = document.querySelector('[data-slot="worker-list"]');
      setResult(JSON.stringify({ ...seen, fromRail: {
        title: view?.querySelector('[data-slot="worker-header"] h2')?.textContent,
        thread: !!view?.querySelector('[data-slot="worker-thread"]'),
        brief: view?.querySelector('[data-slot="worker-brief"]')?.textContent,
        composer: !!view?.querySelector('[data-slot="composer"], [contenteditable], textarea'),
      } }));
    }, 600);
    return () => {
      clearTimeout(listStop);
      clearTimeout(open);
      clearTimeout(stop);
      clearTimeout(snapshot);
      clearTimeout(rail);
      clearTimeout(timer);
    };
  }, []);
  return <TooltipProvider><AgentsPanelContext.Provider value={{ panel, setPanel }}>
    <main className="p-6">
      {Object.keys(tasks).map((id) => <section key={id} data-task={id}><TeamSentence row={{ type: "task", taskId: id, position: 0 }} /></section>)}
      <WorkersSummary conversationId={conversationId} />
      <div data-slot="worker-list"><WorkersTab conversationId={conversationId} /></div>
      <StripOf conversationId={conversationId} />
      <BoardStoreContext.Provider value={idleBoard}>
        <div data-slot="idle-strip"><StripOf conversationId={idleId} /></div>
      </BoardStoreContext.Provider>
      <AgentsPanelContext.Provider value={{ panel: "active", setPanel }}>
        <div data-slot="worker-view"><WorkersTab conversationId={conversationId} /></div>
      </AgentsPanelContext.Provider>
      <AgentsPanelContext.Provider value={{ panel: "completed", setPanel }}>
        <div data-slot="worker-view-done"><WorkersTab conversationId={conversationId} /></div>
      </AgentsPanelContext.Provider>
      <pre id="worker-activity-result">{result}</pre>
    </main>
  </AgentsPanelContext.Provider></TooltipProvider>;
}
/** The Workers strip as the composer's slot shows it, while it has something to show. */
function StripOf({ conversationId: id }: { conversationId: string }) {
  const view = useWorkersStrip(id);
  return view ? <WorkersStrip conversationId={id} view={view} /> : null;
}
createRoot(document.getElementById("root")!).render(<Fixture />);
