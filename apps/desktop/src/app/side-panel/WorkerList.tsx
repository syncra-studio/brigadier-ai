import { useState } from "react";

import { rowFinished, rowWaiting } from "@/app/conversation/activity/workerState";
import { AgentRow } from "@/app/side-panel/AgentRow";
import { WORKERS_LABEL } from "@/app/side-panel/workersLabel";
import { useNow } from "@/hooks/use-now";
import type { WorkerRow } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { useWorkerRows } from "@/state/cliThread";

/**
 * One section of the list, a page at a time: the first `page` rows, then "Show N more" for
 * the next page. The Done section counts its rows in its title.
 */
function AgentSection({
  title,
  rows,
  page,
  counted,
  trailing,
  empty,
  className,
}: {
  title: string;
  rows: readonly WorkerRow[];
  page: number;
  counted?: boolean;
  trailing?: string | undefined;
  empty?: string;
  className?: string;
}) {
  const [pages, setPages] = useState(1);
  if (rows.length === 0 && !empty) return null;
  const shown = rows.slice(0, page * pages);
  const hidden = rows.length - shown.length;
  return (
    <section className={cn("flex flex-col", className)}>
      <h3 className="text-muted-foreground mb-2 flex items-center gap-2 px-2 text-label">
        <span className="flex-1">{counted ? `${title} · ${rows.length}` : title}</span>
        {trailing && <span className="text-xs tabular-nums">{trailing}</span>}
      </h3>
      {rows.length === 0 ? (
        <p className="text-muted-foreground px-2 text-sm">{empty}</p>
      ) : (
        <ul className="flex flex-col gap-0.5">
          {shown.map((row) => (
            <AgentRow key={row.id} row={row} />
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

/** Rows a section shows before "Show N more": 4 active and 10 done. */
const ACTIVE_PAGE = 4;
const DONE_PAGE = 10;

/** Every worker of the session, ordered by recency. */
export function WorkerList({ conversationId }: { conversationId: string }) {
  // Newest first, as the engine sends them; at work first, then finished.
  const rows = useWorkerRows(conversationId);
  const now = useNow(30_000);
  const active = rows.filter((row) => !rowFinished(row, now));
  const finished = rows.filter((row) => rowFinished(row, now));
  const waiting = active.filter((row) => rowWaiting(row, now)).length;
  return (
    <div
      className="min-h-0 flex-1 overflow-y-auto px-3 py-5"
    >
      <div className="max-w-thread mx-auto flex w-full flex-col">
        <AgentSection
          title="Active"
          counted
          rows={active}
          page={ACTIVE_PAGE}
          trailing={waiting > 0 ? `${waiting} waiting` : undefined}
          empty={`No active ${WORKERS_LABEL.toLowerCase()}`}
        />
        <AgentSection
          title="Done"
          rows={finished}
          page={DONE_PAGE}
          counted
          className="mt-6"
        />
      </div>
    </div>
  );
}
