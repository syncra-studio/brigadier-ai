/**
 * The Workers strip with a single worker, waiting on the user, as the composer's slot shows it
 * (#735): the pointer is moved over its words as the user's is, and what its row looks like
 * before and under the pointer (cursor, words' colour, fill) goes into `#workers-strip-hover-result`.
 */
import { createRoot } from "react-dom/client";
import { mockIPC } from "@/ipc/nativeMocks";

import { useWorkersStrip, WorkersStrip } from "@/app/conversation/activity/WorkersStrip";
import { AgentsPanelContext } from "@/app/conversation/WorkerChip";
import { TooltipProvider } from "@/components/overlays";
import night from "@/fixtures/boards/overnight-2026-10-03.json";
import { fixtureInput } from "@/fixtures/input";
import { rowOf, showRows } from "@/fixtures/workerRows";
import type { Task } from "@/ipc/generated";
import { emptyBoard, useBoard } from "@/state/board";

const conversationId = "strip-hover-fixture";
const base = Object.values(night.tasks)[0] as unknown as Task;
const task: Task = {
  ...base, id: "lone", number: 1, title: "Check the build", conversationId, state: "running", blockedReason: null, role: null,
  attempts: [], requestId: "r1", gateLink: null, candidate: null, report: null, quotaWait: null, createdAtMs: 8000, updatedAtMs: 80000,
};
useBoard.setState({ board: { ...emptyBoard(conversationId), loaded: true, tasks: { lone: task } } });
showRows(conversationId, [rowOf(task, { working: true, waiting: [{ kind: "question", what: "Which port?", card: null }] })]);
mockIPC(() => null);

function Strip() {
  const view = useWorkersStrip(conversationId);
  return view ? <WorkersStrip conversationId={conversationId} view={view} /> : null;
}

createRoot(document.getElementById("root")!).render(
  <TooltipProvider>
    <AgentsPanelContext.Provider value={{ panel: undefined, setPanel: () => {} }}>
      <main className="bg-background text-foreground w-96 p-6"><Strip /></main>
    </AgentsPanelContext.Provider>
  </TooltipProvider>,
);

const pause = (ms = 100) => new Promise((resolve) => setTimeout(resolve, ms));
const row = () => document.querySelector<HTMLElement>('[data-slot="workers-strip-row"]');
/** The row's line: its glyph, its name, a dot, then what it does. */
const parts = () => row()!.firstElementChild!.children;
/** How the row looks now. */
function look() {
  const button = row()!;
  const name = parts()[1] as HTMLElement;
  const words = parts()[3] as HTMLElement;
  return {
    cursor: getComputedStyle(button).cursor,
    fill: getComputedStyle(button).backgroundColor,
    name: getComputedStyle(name).color,
    words: getComputedStyle(words).color,
  };
}

const seen: Record<string, unknown> = {};
try {
  for (let tries = 0; tries < 80 && !row(); tries += 1) await pause();
  if (!row()) throw new Error("no strip row");
  seen.rows = document.querySelectorAll('[data-slot="workers-strip-row"]').length;
  // Away from the strip first, then over its words.
  await fixtureInput({ type: "pointer", x: 2, y: 2 });
  await pause(200);
  seen.before = look();
  const name = parts()[1]!.getBoundingClientRect();
  await fixtureInput({ type: "pointer", x: name.left + name.width / 2, y: name.top + name.height / 2 });
  await pause(400);
  seen.hovered = look();
} catch (error) {
  seen.error = String(error);
}
const result = document.createElement("pre");
result.id = "workers-strip-hover-result";
result.textContent = JSON.stringify(seen);
document.body.append(result);
