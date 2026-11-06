import { type FC, useContext } from "react";

import { WORKERS_LABEL } from "@/app/side-panel/workersLabel";
import { Counts } from "@/components/shared";
import { nowInLine, workerNow } from "@/app/conversation/activity/workerNow";
import { WorkerControls } from "@/app/conversation/activity/StopWorker";
import { endedWord, finishedWords, rowOnHand } from "@/app/conversation/activity/workerState";
import { AgentsPanelContext, useWorkerName } from "@/app/conversation/WorkerChip";
import { SummaryRowButton, SummarySection, WorkerGlyph } from "@/components/conversation";
import { useNow } from "@/hooks/use-now";
import type { WorkerRow } from "@/ipc/generated";
import { useWorkerRows } from "@/state/cliThread";
import { isNativeWorker } from "@/state/nativeWorkers";

/**
 * The session's worker summary: its workers as their CLIs and git show them (S14), its CLI's own
 * subagents among them (D50).
 */

/** How many workers at work still get a line each; more read as one row. */
const WORKER_LINES = 3;

/** How often the rows read the clock: a worker turns quiet after minutes, not seconds. */
const TICK_MS = 30_000;

/**
 * A worker at work: its glyph, its name, in a soft word what it does while it works ("running
 * pnpm build", shimmering) or its state ("needs you", "paused"), and its +N −N against the
 * session's branch, under its pause/resume and End on hover (D199). The row opens the worker's
 * panel. With neither to offer (it finished; a subagent that can't be ended), its +N −N stays.
 */
const WorkerLineRow: FC<{ row: WorkerRow; now: number }> = ({ row, now }) => {
  const name = useWorkerName(row.id);
  const { setPanel } = useContext(AgentsPanelContext);
  const live = workerNow(row, now);
  const onHand = rowOnHand(row, now);
  // A finished one reads only done or ended here; a paused one, paused (D198, D199).
  const state = onHand ? live : { kind: live.kind, word: endedWord(live.kind) };
  const stat = row.diff;
  const controls = onHand && !row.gone && (!isNativeWorker(row) || row.native.backgroundId !== null);
  return (
    <SummaryRowButton
      data-slot="workers-summary-worker"
      data-worker-state={state.kind}
      title={name ?? undefined}
      onClick={() => setPanel(row.id)}
      icon={<span className="flex h-lh items-center"><WorkerGlyph taskId={row.id} className="size-4" /></span>}
      meta={stat && stat.insertions + stat.deletions > 0 && (
        <Counts data-slot="workers-summary-changes" insertions={stat.insertions} deletions={stat.deletions} className="text-2xs" />
      )}
      action={controls ? <WorkerControls row={row} name={name ?? "this worker"} /> : undefined}
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
  const rows = useWorkerRows(conversationId);
  const now = useNow(rows.length > 0 ? TICK_MS : null);
  const { panel, setPanel } = useContext(AgentsPanelContext);
  if (!rows.length) return null;
  const active = rows.filter((row) => rowOnHand(row, now));
  const finished = finishedWords(rows, now);
  const avatars = active.length ? active : rows;
  // A few at work get a line each; so does a lone worker that finished, by its name (D104).
  const lined = active.length > 0 && active.length <= WORKER_LINES ? active : rows.length === 1 ? rows : [];
  return (
    <SummarySection foldKey="workers" title={WORKERS_LABEL} aria-label="Session workers">
      {lined.length > 0 ? (
        <>
          {lined.map((row) => <WorkerLineRow key={row.id} row={row} now={now} />)}
          {finished && lined === active && (
            <SummaryRowButton
              aria-label="Open workers"
              data-slot="workers-summary"
              data-state={panel !== undefined ? "open" : "closed"}
              onClick={() => setPanel(null)}
              muted
              icon={<WorkerGlyph taskId={rows.find((row) => !rowOnHand(row, now))!.id} className="size-4" />}
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
