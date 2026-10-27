import { type FC, useContext, useMemo } from "react";

import { WORKERS_LABEL } from "@/app/side-panel/workersLabel";
import { Counts } from "@/components/shared";
import { nowInLine, workerNow } from "@/app/conversation/activity/workerNow";
import { finishedWords, rowFinished } from "@/app/conversation/activity/workerState";
import { AgentsPanelContext, useWorkerName } from "@/app/conversation/WorkerChip";
import { showWorkerSource } from "@/app/side-panel/sourceTarget";
import { SummaryRowButton, SummarySection, WorkerGlyph } from "@/components/conversation";
import { useNow } from "@/hooks/use-now";
import type { WorkerRow } from "@/ipc/generated";
import { useWorkerRows } from "@/state/cliThread";
import { isNativeWorker } from "@/state/nativeWorkers";

/**
 * The session's worker summary: its own workers as their CLIs and git show them (S14). Its CLI's
 * subagents are not workers: they show in their own strip on the composer and in History.
 */

/** How many workers at work still get a line each; more read as one row. */
const WORKER_LINES = 3;

/** How often the rows read the clock: a worker turns quiet after minutes, not seconds. */
const TICK_MS = 30_000;

/**
 * A worker at work: its glyph, its name, in a soft word what it does while it works ("running
 * pnpm build", shimmering) or its state ("needs you"), and its +N −N against the session's branch.
 * The row opens the worker's panel; its +N −N, the right sidebar's Source on the worker's changes.
 */
const WorkerLineRow: FC<{ conversationId: string; row: WorkerRow; now: number }> = ({ conversationId, row, now }) => {
  const name = useWorkerName(row.id);
  const { setPanel } = useContext(AgentsPanelContext);
  const state = workerNow(row, now);
  const stat = row.diff;
  return (
    <SummaryRowButton
      data-slot="workers-summary-worker"
      data-worker-state={state.kind}
      title={name ?? undefined}
      onClick={() => setPanel(row.id)}
      icon={<span className="flex h-lh items-center"><WorkerGlyph taskId={row.id} className="size-4" /></span>}
      action={stat && stat.insertions + stat.deletions > 0 && (
        <button
          type="button"
          data-slot="workers-summary-changes"
          aria-label={`Show ${name ?? "the worker"}'s changes in Source`}
          title="Show changes in Source"
          onClick={() => showWorkerSource(conversationId, row.id)}
          className="rounded-sm text-xs hover:bg-foreground/8 focus-visible:ring-ring cursor-pointer outline-none focus-visible:ring-1"
        >
          <Counts insertions={stat.insertions} deletions={stat.deletions} />
        </button>
      )}
      actionVisible
    >
      <span className="block truncate">
        {name}
        <span className={state.kind === "working" ? "text-muted-foreground shimmer" : "text-muted-foreground"}> {nowInLine(state)}</span>
      </span>
    </SummaryRowButton>
  );
};

/**
 * The session's workers in the context card: one row of avatars with "N working" or "N done"
 * (the 10-07 row), or, while a few work, a line each. A click opens the Workers panel, or the
 * worker's own; what a worker runs shows there, not here.
 */
export function WorkersSummary({ conversationId }: { conversationId: string }) {
  const all = useWorkerRows(conversationId);
  const rows = useMemo(() => all.filter((row) => !isNativeWorker(row)), [all]);
  const now = useNow(rows.length > 0 ? TICK_MS : null);
  const { panel, setPanel } = useContext(AgentsPanelContext);
  if (!rows.length) return null;
  const active = rows.filter((row) => !rowFinished(row, now));
  const finished = finishedWords(rows, now);
  const avatars = active.length ? active : rows;
  const lines = active.length > 0 && active.length <= WORKER_LINES;
  return (
    <SummarySection foldKey="workers" title={WORKERS_LABEL} aria-label="Session workers">
      {lines ? (
        <>
          {active.map((row) => <WorkerLineRow key={row.id} conversationId={conversationId} row={row} now={now} />)}
          {finished && (
            <SummaryRowButton
              aria-label="Open workers"
              data-slot="workers-summary"
              data-state={panel !== undefined ? "open" : "closed"}
              onClick={() => setPanel(null)}
              muted
              icon={<WorkerGlyph taskId={rows.find((row) => rowFinished(row, now))!.id} className="size-4" />}
            >
              {finished}
            </SummaryRowButton>
          )}
        </>
      ) : (
        <SummaryRowButton
          aria-label="Open workers"
          data-slot="workers-summary"
          // Washed while the Workers tab shows, as on hover.
          data-state={panel !== undefined ? "open" : "closed"}
          onClick={() => setPanel(null)}
          className="gap-1.5"
          icon={
            <span className="flex items-center gap-1.5">
              {avatars.slice(0, 4).map((row) => <WorkerGlyph key={row.id} taskId={row.id} className="size-4" />)}
            </span>
          }
          meta={active.length > 0 && finished ? finished : undefined}
        >
          {active.length ? `${active.length} working` : finished}
        </SummaryRowButton>
      )}
    </SummarySection>
  );
}
