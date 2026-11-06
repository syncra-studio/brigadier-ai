import { type FC, useCallback, useEffect, useState } from "react";

import { useWorkerName } from "@/app/conversation/WorkerChip";
import { FileRow, SourcePanel } from "@/app/side-panel/SourcePanel";
import { SourceHeader, SourceRefresh, SourceRow } from "@/app/side-panel/SourceHeader";
import { reason, useRowOpen, workerRowKey } from "@/app/side-panel/sourceTarget";
import { rowReveal, WorkerGlyph } from "@/components/conversation";
import { TooltipIconButton } from "@/components/controls";
import { FileDocument } from "@/components/icons";
import { Counts } from "@/components/shared";
import { RequestError } from "@/ipc/client";
import type { WorkerRow } from "@/ipc/generated";
import { useWorkerRows } from "@/state/cliThread";
import { openReviewTab } from "@/state/sessionTabs";
import { readWorkerSource, useKeptWorkerSource } from "@/state/workerSourceCache";
import { cn } from "@/lib/utils";

/**
 * A session's Source: its own checkout, and with workers a stacked row for it ("This session")
 * and one for each worker whose worktree git reads. A worker's row is read only: it writes there.
 */
export function SessionSource({ conversationId }: { conversationId: string }) {
  const all = useWorkerRows(conversationId);
  const workers = all.filter((row) => row.diff !== null);
  const target = { type: "conversation", conversationId } as const;
  if (workers.length === 0) return <SourcePanel target={target} />;
  return (
    <div data-slot="source-rows" className="flex min-h-0 flex-1 flex-col">
      <SourceHeader />
      <div className="min-h-0 flex-1 overflow-y-auto pb-2">
        <SourcePanel
          target={target}
          row={{ name: "This session", title: "This session", rowKey: `session:${conversationId}`, startOpen: true }}
        />
        {workers.map((worker) => <WorkerSourceRow key={worker.id} conversationId={conversationId} worker={worker} />)}
      </div>
    </div>
  );
}

/**
 * A worker's changes: what was last read of them at once, then read again on every view, when its
 * +N −N moves, on focus and on Refresh: its branch since it left the session's, committed and not.
 */
function useWorkerSource(conversationId: string, worker: WorkerRow) {
  const taskId = worker.id;
  const source = useKeptWorkerSource(conversationId, taskId);
  const [error, setError] = useState<string | null>(null);
  const [gone, setGone] = useState(false);
  const moved = `${worker.diff?.insertions}:${worker.diff?.deletions}:${worker.working}:${worker.lastEnd}`;
  const read = useCallback((again: boolean) => {
    readWorkerSource(conversationId, taskId, moved, again)
      .then(() => {
        setError(null);
        setGone(false);
      })
      .catch((cause: unknown) => {
        // Gone (closed, its worktree removed): its row goes, quietly.
        if (cause instanceof RequestError && cause.code === "notFound") setGone(true);
        else setError(reason(cause));
      });
  }, [conversationId, taskId, moved]);
  useEffect(() => {
    const onFocus = () => read(false);
    onFocus();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [read]);
  return { source, error, gone, refresh: () => read(true) };
}

/**
 * A worker's row: its name, its branch, Refresh and how many files it changed, with Open Changes
 * over the branch's end on hover; under it the files with their letters and +N −N, each opening
 * its part of the worker's diff in the Review tab. Nothing is staged, committed or discarded here.
 */
const WorkerSourceRow: FC<{ conversationId: string; worker: WorkerRow }> = ({ conversationId, worker }) => {
  const { source, error, gone, refresh } = useWorkerSource(conversationId, worker);
  const [open, setOpen] = useRowOpen(workerRowKey(conversationId, worker.id), false);
  const name = useWorkerName(worker.id) ?? "Worker";
  const files = source?.files ?? [];
  const review = (path?: string) =>
    openReviewTab(conversationId, { type: "worker", taskId: worker.id, ...(path ? { path } : {}), staged: false });
  if (gone) return null;
  return (
    <div data-slot="source-panel" data-scope="worker" className="relative flex shrink-0 flex-col">
      <div className="group/row">
        <SourceRow
          open={open}
          onToggle={() => setOpen(!open)}
          icon={<WorkerGlyph taskId={worker.id} className="size-icon-sm shrink-0" />}
          name={name}
          title={name}
          branch={source?.branch ?? null}
          {...(source ? { branchTitle: `${source.branch ?? "…"} into ${source.base}` } : {})}
        >
          <span className="relative flex items-center">
            {files.length > 0 && (
              <span className={cn(rowReveal, "row-backing row-backing-pane absolute inset-y-0 end-full flex items-center ps-4")}>
                <TooltipIconButton tooltip="Open Changes" size="icon-xs" onClick={() => review()}>
                  <FileDocument />
                </TooltipIconButton>
              </span>
            )}
            <SourceRefresh onRefresh={refresh} />
            {files.length > 0 && <span data-slot="source-row-count" className="bg-muted text-muted-foreground ms-1 me-2 rounded-full px-1.5 text-xs tabular-nums">{files.length}</span>}
          </span>
        </SourceRow>
      </div>
      {open && (
        <div className="ps-6 pe-2 pb-2">
          {error && !source && <p className="text-destructive p-2 text-sm">{error}</p>}
          {source && files.length === 0 && <p className="text-muted-foreground p-2 text-sm">No changes yet</p>}
          {files.length > 0 && (
            <ul>
              {files.map((file) => (
                <FileRow
                  key={file.path}
                  file={{ path: file.path, oldPath: file.from, status: file.status }}
                  onOpen={() => review(file.path)}
                  counts={!file.binary && <Counts insertions={file.insertions} deletions={file.deletions} className="text-2xs opacity-70" />}
                />
              ))}
            </ul>
          )}
        </div>
      )}
    </div>
  );
};
