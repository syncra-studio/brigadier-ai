/** Tests lifecycle rows with no commands, internal ids or diff totals, and the Workers strip. */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { TeamSentence } from "@/app/conversation/activity/TeamSentence";
import { useWorkersStrip, WorkersStrip } from "@/app/conversation/activity/WorkersStrip";
import { WorkersSummary } from "@/app/conversation/WorkerSummary";
import { AgentsPanelContext } from "@/app/conversation/WorkerChip";
import { WorkersTab } from "@/app/conversation/Agents";
import { TooltipProvider } from "@/components/overlays/tooltip";
import night from "@/fixtures/boards/overnight-2026-10-03.json";
import type { Task } from "@/ipc/generated";
import { emptyBoard, useBoard } from "@/state/board";
const base = Object.values(night.tasks)[0] as unknown as Task;
// One request, not answered yet: its finished worker stays in the strip.
const conversationId = "worker-fixture";
const task = (id: string, number: number, state: Task["state"], blockedReason: string | null = null): Task => ({
  ...base, id, number, title: `Check ${id}`, conversationId, state, blockedReason, role: null, phase: null,
  run: null, attempts: [], requestId: "r1", gateLink: null, candidate: null, report: null, quotaWait: null,
  createdAtMs: 8000, updatedAtMs: 80000,
});
const tasks = { active: task("active", 1, "running"), blocked: task("blocked", 2, "blocked", "Waiting for plan review"),
  queued: task("queued", 3, "queued"), completed: task("completed", 4, "done"),
  // Stopped before it landed: it counts as stopped, never as done (BUILD-PLAN A7).
  stopped: task("stopped", 5, "stopped") };
useBoard.setState({ board: { ...emptyBoard(conversationId), loaded: true, tasks } });
// What the page asked the daemon (a worker's Stop sends `stopTask`).
const sent: string[] = [];
mockIPC((cmd, args) => {
  sent.push(JSON.stringify({ cmd, args }));
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
    const timer = setTimeout(() => setResult(JSON.stringify({
      thread: Object.fromEntries([...document.querySelectorAll('main > section[data-task]')].map((row) => [row.getAttribute("data-task"), row.textContent])),
      summary: document.querySelector('[aria-label="Session workers"]')?.textContent,
      list: document.querySelector('[data-slot="worker-list"]')?.textContent,
      strip: document.querySelector('[data-slot="workers-strip"]')?.textContent,
      stripRows: document.querySelectorAll('[data-slot="workers-strip-row"]').length,
      stripStops: [...document.querySelectorAll<HTMLButtonElement>('[data-slot="workers-strip"] [data-slot="worker-stop"]')].map((button) => button.getAttribute("aria-label")),
      listStops: [...document.querySelectorAll<HTMLButtonElement>('[data-slot="worker-list"] [data-slot="worker-stop"]')].map((button) => button.getAttribute("aria-label")),
      headerStop: document.querySelector('[data-slot="worker-view"] [data-slot="worker-header"] [data-slot="worker-stop"]')?.getAttribute("aria-label"),
      doneHeaderStop: !!document.querySelector('[data-slot="worker-view-done"] [data-slot="worker-stop"]'),
      sent: sent.filter((call) => call.includes("stopTask")),
      avatars: [...document.querySelectorAll('[data-slot="task-row"] span[aria-hidden] svg')].map((svg) => svg.getBoundingClientRect().width),
    })), 400);
    return () => {
      clearTimeout(listStop);
      clearTimeout(open);
      clearTimeout(stop);
      clearTimeout(timer);
    };
  }, []);
  return <TooltipProvider><AgentsPanelContext.Provider value={{ panel, setPanel }}>
    <main className="p-6">
      {Object.keys(tasks).map((id) => <section key={id} data-task={id}><TeamSentence row={{ type: "task", taskId: id, position: 0 }} /></section>)}
      <WorkersSummary conversationId={conversationId} />
      <div data-slot="worker-list"><WorkersTab conversationId={conversationId} /></div>
      <StripOf conversationId={conversationId} />
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
