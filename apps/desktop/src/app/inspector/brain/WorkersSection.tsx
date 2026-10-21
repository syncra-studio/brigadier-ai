import { useEffect } from "react";

import { BrainSection } from "@/app/inspector/brain/BrainSection";
import { Badge } from "@/components/controls";
import { brainUseText, lowBrainUse } from "@/lib/brainUse";
import { inspectedConversation } from "@/state/actions";
import { unwatchSessionTasks, useBrain, watchSessionTasks } from "@/state/brain";
import { useApp } from "@/state/store";

/**
 * How the inspected session's workers used this Brain: their queries, the facts their briefs
 * carried, what they wrote back and their calls. A long task that neither asked nor taught it
 * is marked "Low Brain use": a gap to fix (SESSION-FIXES-PLAN.md Q6.6). The session is the one
 * Settings was opened from; its tasks are read once and kept current by task events.
 */
export function WorkersSection({ projectId }: { projectId: string }) {
  const sessionId = useApp((s) => {
    const open = inspectedConversation(s);
    return open?.projectId === projectId ? open.id : null;
  });
  const tasks = useBrain((s) =>
    sessionId !== null && s.sessionTasks?.conversationId === sessionId ? s.sessionTasks.tasks : null,
  );
  // Read on showing; the bridge's resync reads again on reconnecting or after missed events.
  useEffect(() => {
    if (sessionId === null) return undefined;
    watchSessionTasks(sessionId).catch((cause: unknown) => {
      console.error("reading the session's workers failed", cause);
    });
    return () => unwatchSessionTasks(sessionId);
  }, [sessionId]);
  if (!tasks) return null;
  const workers = Object.values(tasks)
    .filter((task) => task.brain)
    .toSorted((a, b) => a.number - b.number);
  if (workers.length === 0) return null;
  return (
    <BrainSection title="Workers in this session">
      <ul className="divide-border flex flex-col divide-y">
        {workers.map((task) => (
          <li key={task.id} data-task={`task-${task.number}`} className="flex flex-col gap-0.5 py-1.5">
            <div className="flex items-center gap-2">
              <span className="min-w-0 flex-1 truncate font-medium">
                task-{task.number} {task.title}
              </span>
              {lowBrainUse(task.brain) && <Badge variant="warning">Low Brain use</Badge>}
            </div>
            <p className="text-muted-foreground tabular-nums">{brainUseText(task.brain)}</p>
          </li>
        ))}
      </ul>
    </BrainSection>
  );
}
