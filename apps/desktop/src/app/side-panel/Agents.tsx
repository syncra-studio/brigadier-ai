import { type ReactNode, useContext, useState } from "react";

import { workerStateOf } from "@/app/conversation/activity/workerState";
import { TaskDetails } from "@/app/side-panel/TaskDetails";
import { MarkdownBlock } from "@/components/conversation";
import {
  AgentsPanelContext,
  useWorkerName,
  useWorkerText,
} from "@/app/conversation/WorkerChip";
import { WorkerHeader } from "@/app/side-panel/WorkerHeader";
import { WorkerList } from "@/app/side-panel/WorkerList";
import { WorkerThread } from "@/app/side-panel/WorkerThread";
import { useNow } from "@/hooks/use-now";
import type { Task, WorkerRow } from "@/ipc/generated";
import { modelName, useModelGroups } from "@/lib/setup";
import { useBoard } from "@/state/board";
import { isNativeWorker } from "@/state/nativeWorkers";
import { useWorkerRows } from "@/state/cliThread";

export { WORKERS_LABEL } from "@/app/side-panel/workersLabel";

/** The session's flat worker list and read-only worker threads. */

/** A worker's details page: what Brigadier keeps of the task it was started for. */
function WorkerDetails({ task, model }: { task: Task; model: string }) {
  const instructions = useWorkerText(task.spec);
  return (
    <div data-slot="worker-details" data-selectable className="min-h-0 min-w-0 flex-1 overflow-y-auto px-4 py-5 wrap-anywhere">
      <div className="flex flex-col gap-4">
        <h3 className="text-sm font-medium">Details</h3>
        <TaskDetails task={task} model={model} inThread detailsPage />
        <section className="flex flex-col gap-2">
          <h3 className="text-sm font-medium">Instructions</h3>
          <MarkdownBlock text={instructions} />
        </section>
      </div>
    </div>
  );
}

/** One worker: its glyph, name, model and state in the header, from its row; its thread below. */
function WorkerDetail({ conversationId, row, headerActions }: { conversationId: string; row: WorkerRow; headerActions?: ReactNode }) {
  const { setPanel } = useContext(AgentsPanelContext);
  const name = useWorkerName(row.id);
  const [details, setDetails] = useState(false);
  // Its details page is the task's it was started for, while the board holds it.
  const task = useBoard((s) => s.board?.tasks[row.id]);
  const groups = useModelGroups();
  const now = useNow(30_000);
  const model = modelName(groups, { provider: row.agent, model: row.model, effort: null });
  const state = workerStateOf(row, now).word;
  return (
    <section aria-label={`Worker: ${name ?? row.title}`} className="flex min-h-0 min-w-0 flex-1 flex-col">
      <WorkerHeader
        row={row}
        name={name}
        model={model}
        state={state}
        details={details}
        onBack={() => details ? setDetails(false) : setPanel(null)}
        onToggleDetails={task || isNativeWorker(row) ? () => setDetails((open) => !open) : undefined}
        headerActions={headerActions}
      />
      {details && isNativeWorker(row) ? (
        <div data-slot="worker-details" data-selectable className="min-h-0 min-w-0 flex-1 overflow-y-auto px-4 py-5 wrap-anywhere">
          <h3 className="mb-4 text-sm font-medium">Instructions</h3>
          <MarkdownBlock text={row.native.prompt || "The CLI did not provide this agent’s instructions."} />
        </div>
      ) : details && task ? <WorkerDetails task={task} model={model} /> : <WorkerThread key={row.id} conversationId={conversationId} row={row} />}
    </section>
  );
}

/** The Workers tab of the side panel: every worker of the session, or one of them working. */
export function WorkersTab({ conversationId, headerActions }: { conversationId: string; headerActions?: ReactNode }) {
  const { panel } = useContext(AgentsPanelContext);
  const selected = useWorkerRows(conversationId).find((row) => row.id === panel);
  if (selected) return <WorkerDetail key={selected.id} conversationId={conversationId} row={selected} headerActions={headerActions} />;
  return <WorkerList conversationId={conversationId} />;
}
