/**
 * A mockup of the Workers strip's rows (no daemon), in the app's window from `flow.tsx`: a Pencil
 * design worker, one on a Linear tool and one running tests. Open with `?slot=1&approvals=0`;
 * `after=1` shows the planned rows: an integration's live line names its action, and a worker's
 * progress note shows under it. Without it, the rows as the app words them today.
 */
// oxlint-disable-next-line import/no-unassigned-import -- It renders the app window this page fills.
import "@/fixtures/flow";

import { liveActivity } from "@/app/conversation/activity/words";
import type { Task } from "@/ipc/generated";
import { useActivity } from "@/state/activity";
import { useBoard } from "@/state/board";

const after = new URLSearchParams(location.search).has("after");
const now = Date.now();
const minutes = (count: number) => now - count * 60_000;
const board = useBoard.getState().board!;
const lead = board.tasks.t2!;
const worker = (id: string, number: number, title: string, startedAtMs: number, kind: Task["kind"] = "implement"): Task =>
  ({ ...lead, id, number, title, kind, phase: null, role: null, state: "running", position: 12 + number, createdAtMs: startedAtMs, updatedAtMs: now });
const tasks = {
  ...Object.fromEntries(Object.entries(board.tasks).filter(([, task]) => task.state !== "running")),
  t2: worker("t2", 2, "Landing page design", minutes(47)),
  t5: worker("t5", 5, "Triage open bugs", minutes(2), "scout"),
  t6: worker("t6", 6, "CSV exporter", minutes(6)),
};
// Today's words come from the real `liveActivity`; after, the integration's action is named.
const pencil = { name: "mcp__pencil__batch_design", input: JSON.stringify({ filePath: "landing.pen" }) };
const linear = { name: "mcp__linear__search_issues", input: JSON.stringify({ query: "export" }) };
const tests = { command: "pnpm test exporter" };
useBoard.setState({ board: { ...board, tasks,
  activity: after
    ? { t2: "Pencil: batch design", t5: "Linear: search issues", t6: liveActivity(tests) }
    : { t2: liveActivity(pencil), t5: liveActivity(linear), t6: liveActivity(tests) },
  // The Linear worker posted no note: its row stays one line.
  notes: after ? { t2: "Scene 4 of 12: pricing section", t6: "Step 3 of 5: wiring the exporter" } : {},
  orchestratorSteps: board.orchestratorSteps.filter((step) => step.kind.type !== "ciWatch" && step.kind.type !== "mergeHeld"),
} });
useActivity.setState((s) => ({ byConversation: { ...s.byConversation, "flow-session": { ...s.byConversation["flow-session"]!,
  tasks: Object.fromEntries(Object.values(tasks).map((task) => [task.id, task.state])) } } }));

// Open the strip, as a click on its count does.
const open = setInterval(() => {
  const toggle = document.querySelector<HTMLButtonElement>('[data-slot="workers-strip"] button[aria-expanded="false"]');
  if (!toggle) return;
  toggle.click();
  clearInterval(open);
}, 50);
