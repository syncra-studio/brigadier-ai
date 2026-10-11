/** Tests lifecycle rows with no commands, internal ids or diff totals, and the Workers strip. */
import { mockIPC } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { TeamSentence } from "@/app/conversation/activity/TeamSentence";
import { useWorkersStrip, WorkersStrip } from "@/app/conversation/activity/WorkersStrip";
import { WorkersSummary } from "@/app/conversation/WorkerSummary";
import { AgentsPanelContext } from "@/app/conversation/WorkerChip";
import { WorkersTab } from "@/app/conversation/Agents";
import { TooltipProvider } from "@/components/ui/tooltip";
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
  queued: task("queued", 3, "queued"), completed: task("completed", 4, "done") };
useBoard.setState({ board: { ...emptyBoard(conversationId), loaded: true, tasks } });
mockIPC(() => null);
function Fixture() {
  const [result, setResult] = useState("");
  const [panel, setPanel] = useState<string | null | undefined>();
  useEffect(() => {
    // Open the strip, as a click on its count does.
    const open = setTimeout(() => document.querySelector<HTMLButtonElement>('[data-slot="workers-strip"] button[aria-expanded]')?.click(), 100);
    const timer = setTimeout(() => setResult(JSON.stringify({
      thread: Object.fromEntries([...document.querySelectorAll('main > section[data-task]')].map((row) => [row.getAttribute("data-task"), row.textContent])),
      summary: document.querySelector('[data-slot="workers-summary"]')?.textContent,
      list: document.querySelector('[data-slot="worker-list"]')?.textContent,
      strip: document.querySelector('[data-slot="workers-strip"]')?.textContent,
      stripRows: document.querySelectorAll('[data-slot="workers-strip-row"]').length,
      avatars: [...document.querySelectorAll('[data-slot="task-row"] span[aria-hidden] svg')].map((svg) => svg.getBoundingClientRect().width),
    })), 400);
    return () => {
      clearTimeout(open);
      clearTimeout(timer);
    };
  }, []);
  return <TooltipProvider><AgentsPanelContext.Provider value={{ panel, setPanel }}>
    <main className="p-6">
      {Object.keys(tasks).map((id) => <section key={id} data-task={id}><TeamSentence row={{ type: "task", taskId: id, position: 0 }} /></section>)}
      <WorkersSummary conversationId={conversationId} />
      <div data-slot="worker-list"><WorkersTab conversationId={conversationId} /></div>
      <StripOf conversationId={conversationId} />
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
