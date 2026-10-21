import { useMemo, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { workerDone, workerWorking } from "@/app/conversation/workerPresentation";
import { AgentRow } from "@/app/side-panel/AgentRow";
import { WORKERS_LABEL } from "@/app/side-panel/workersLabel";
import { cn } from "@/lib/utils";
import { useBoard } from "@/state/board";

/**
 * One section of the list, a page at a time: the first `page` rows, then "Show N more" for
 * the next page. The Done section counts its rows in its title.
 */
function AgentSection({
  title,
  ids,
  page,
  counted,
  trailing,
  empty,
  className,
}: {
  title: string;
  ids: readonly string[];
  page: number;
  counted?: boolean;
  trailing?: string | undefined;
  empty?: string;
  className?: string;
}) {
  const [pages, setPages] = useState(1);
  if (ids.length === 0 && !empty) return null;
  const shown = ids.slice(0, page * pages);
  const hidden = ids.length - shown.length;
  return (
    <section className={cn("flex flex-col", className)}>
      <h3 className="text-muted-foreground mb-2 flex items-center gap-2 px-2 text-label">
        <span className="flex-1">{counted ? `${title} · ${ids.length}` : title}</span>
        {trailing && <span className="text-xs tabular-nums">{trailing}</span>}
      </h3>
      {ids.length === 0 ? (
        <p className="text-muted-foreground px-2 text-sm">{empty}</p>
      ) : (
        <ul className="flex flex-col gap-0.5">
          {shown.map((id) => (
            <AgentRow key={id} taskId={id} />
          ))}
        </ul>
      )}
      {hidden > 0 && (
        <button
          type="button"
          onClick={() => setPages(pages + 1)}
          className="text-muted-foreground hover:text-foreground ms-9 mt-1 self-start px-2 py-1 text-sm transition-colors"
        >
          Show {Math.min(hidden, page)} more
        </button>
      )}
    </section>
  );
}

/** Active workers first, then finished workers, newest first within each group. */
export function useWorkerIds(conversationId: string): { active: string[]; finished: string[] } {
  const ids = useBoard(
    useShallow((s) => {
      if (s.board?.conversationId !== conversationId) return [];
      const tasks = Object.values(s.board.tasks).toSorted((a, b) => b.createdAtMs - a.createdAtMs || b.number - a.number);
      // `|` separates the two lists.
      return [
        ...tasks.filter((task) => !workerDone(task)).map((task) => task.id),
        "|",
        ...tasks.filter(workerDone).map((task) => task.id),
      ];
    }),
  );
  return useMemo(() => {
    const split = ids.indexOf("|");
    return { active: ids.slice(0, Math.max(split, 0)), finished: ids.slice(split + 1) };
  }, [ids]);
}

/** Rows a section shows before "Show N more": 4 active and 10 done. */
const ACTIVE_PAGE = 4;
const DONE_PAGE = 10;

/** Every worker of the session, ordered by recency. */
export function WorkerList({ conversationId }: { conversationId: string }) {
  const lists = useWorkerIds(conversationId);
  const waiting = useBoard(
    (s) =>
      s.board?.conversationId === conversationId
        ? Object.values(s.board.tasks).filter((task) => !workerDone(task) && !workerWorking(task))
            .length
        : 0,
  );
  return (
    <div
      className="min-h-0 flex-1 overflow-y-auto px-3 py-5"
    >
      <div className="max-w-thread mx-auto flex w-full flex-col">
        <AgentSection
          title="Active"
          counted
          ids={lists.active}
          page={ACTIVE_PAGE}
          trailing={waiting > 0 ? `${waiting} waiting` : undefined}
          empty={`No active ${WORKERS_LABEL.toLowerCase()}`}
        />
        <AgentSection
          title="Done"
          ids={lists.finished}
          page={DONE_PAGE}
          counted
          className="mt-6"
        />
      </div>
    </div>
  );
}
