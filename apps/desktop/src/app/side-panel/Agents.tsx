import { type ReactNode, useContext, useState } from "react";

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
import { effortLabel } from "@/components/shared";
import type { Task } from "@/ipc/generated";
import { modelName, useModelGroups } from "@/lib/setup";
import { useBoard } from "@/state/board";

export { WORKERS_LABEL } from "@/app/side-panel/workersLabel";

/** The session's flat worker list and read-only worker threads. */

/** One worker: its glyph, name and model in the header, its thread below. */
function WorkerDetail({ task, headerActions }: { task: Task; headerActions?: ReactNode }) {
  const { setPanel } = useContext(AgentsPanelContext);
  const name = useWorkerName(task.id);
  const [details, setDetails] = useState(false);
  const instructions = useWorkerText(task.spec);
  const groups = useModelGroups();
  const choice = task.route.choice;
  const model = `${modelName(groups, choice)}${choice.effort ? ` · ${effortLabel(choice.effort)}` : ""}`;
  return (
    <section aria-label={`Worker: ${name ?? task.title}`} className="flex min-h-0 min-w-0 flex-1 flex-col">
      <WorkerHeader
        task={task}
        name={name}
        model={model}
        details={details}
        onBack={() => details ? setDetails(false) : setPanel(null)}
        onToggleDetails={() => setDetails((open) => !open)}
        headerActions={headerActions}
      />
      {details ? <div data-slot="worker-details" data-selectable className="min-h-0 min-w-0 flex-1 overflow-y-auto px-4 py-5 wrap-anywhere">
        <div className="flex flex-col gap-4">
          <h3 className="text-sm font-medium">Details</h3>
          <TaskDetails task={task} model={model} inThread detailsPage />
          <section className="flex flex-col gap-2">
            <h3 className="text-sm font-medium">Instructions</h3>
            <MarkdownBlock text={instructions} />
          </section>
        </div>
      </div> : <WorkerThread key={task.id} task={task} />}
    </section>
  );
}

/** The Workers tab of the side panel: every worker of the session, or one of them working. */
export function WorkersTab({ conversationId, headerActions }: { conversationId: string; headerActions?: ReactNode }) {
  const { panel } = useContext(AgentsPanelContext);
  const selected = useBoard((s) => (panel ? s.board?.tasks[panel] : undefined));
  if (selected) return <WorkerDetail key={selected.id} task={selected} headerActions={headerActions} />;
  return <WorkerList conversationId={conversationId} />;
}
