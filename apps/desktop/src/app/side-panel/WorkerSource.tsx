import { type FC, useCallback, useEffect, useRef, useState } from "react";

import { useWorkerName } from "@/app/conversation/WorkerChip";
import { FileRow, SourcePanel } from "@/app/side-panel/SourcePanel";
import { SourceGroup } from "@/app/side-panel/SourceGroup";
import { SourceHeader, SourceRefresh, SourceRow } from "@/app/side-panel/SourceHeader";
import { reason, setSourceScope, useRowOpen, useSourceScopes, workerRowKey } from "@/app/side-panel/sourceTarget";
import { WorkerGlyph } from "@/components/conversation";
import { TooltipIconButton } from "@/components/controls";
import { FileDocument } from "@/components/icons";
import { Counts } from "@/components/shared";
import { request, RequestError } from "@/ipc/client";
import type { WorkerRow, WorkerSource } from "@/ipc/generated";
import { useWorkerRows } from "@/state/cliThread";
import { openReviewTab } from "@/state/sessionTabs";

/**
 * A session's Source: its own checkout, and with workers a stacked row for it ("This session")
 * and one for each worker whose worktree git reads. A worker's row is read only: it writes there.
 * A worker's +N −N in the context card opens its row.
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
 * A worker's changes, read again when its +N −N moves, on focus and on Refresh: its branch since
 * it left the session's, committed and not.
 */
function useWorkerSource(conversationId: string, worker: WorkerRow) {
  const [source, setSource] = useState<WorkerSource | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [gone, setGone] = useState(false);
  const taskId = worker.id;
  const load = useCallback(() => {
    request({ method: "getWorkerSource", conversationId, taskId })
      .then(({ source: next }) => {
        setSource(next);
        setError(null);
        setGone(false);
      })
      .catch((cause: unknown) => {
        // Gone (closed, its worktree removed): its row goes, quietly.
        if (cause instanceof RequestError && cause.code === "notFound") setGone(true);
        else setError(reason(cause));
      });
  }, [conversationId, taskId]);
  const moved = `${worker.diff?.insertions}:${worker.diff?.deletions}:${worker.working}:${worker.lastEnd}`;
  useEffect(() => {
    load();
    window.addEventListener("focus", load);
    return () => window.removeEventListener("focus", load);
    // `moved` reads the list again when the worker's changes move.
    // oxlint-disable-next-line react/exhaustive-effect-dependencies
  }, [load, moved]);
  return { source, error, gone, load };
}

/**
 * A worker's row: its name, its branch and Refresh, and the files it changed with their letters
 * and +N −N, each opening its part of the worker's diff in the Review tab. Nothing is staged,
 * committed or discarded here.
 */
const WorkerSourceRow: FC<{ conversationId: string; worker: WorkerRow }> = ({ conversationId, worker }) => {
  const { source, error, gone, load } = useWorkerSource(conversationId, worker);
  const [open, setOpen] = useRowOpen(workerRowKey(conversationId, worker.id), false);
  const name = useWorkerName(worker.id) ?? "Worker";
  const files = source?.files ?? [];
  const review = (path?: string) =>
    openReviewTab(conversationId, { type: "worker", taskId: worker.id, ...(path ? { path } : {}), staged: false });
  // Its +N −N in the context card asked for this row: it is opened by then, and comes into view.
  const asked = useSourceScopes((scopes) => scopes[conversationId] === worker.id);
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!asked) return;
    root.current?.scrollIntoView({ block: "nearest" });
    setSourceScope(conversationId, null);
  }, [asked, conversationId]);
  if (gone) return null;
  return (
    <div ref={root} data-slot="source-panel" data-scope="worker" className="relative flex shrink-0 flex-col">
      <SourceRow
        open={open}
        onToggle={() => setOpen(!open)}
        icon={<WorkerGlyph taskId={worker.id} className="size-icon-sm shrink-0" />}
        name={name}
        title={name}
        branch={source?.branch ?? null}
        {...(source ? { branchTitle: `${source.branch ?? "…"} into ${source.base}` } : {})}
      >
        <SourceRefresh onRefresh={load} />
      </SourceRow>
      {open && (
        <div className="ps-6 pe-2 pb-2">
          {error && !source && <p className="text-destructive p-2 text-sm">{error}</p>}
          {source && files.length === 0 && <p className="text-muted-foreground p-2 text-sm">No changes yet</p>}
          {files.length > 0 && (
            <SourceGroup
              title="Changes"
              count={files.length}
              actions={
                <TooltipIconButton tooltip="Open Changes" size="icon-xs" onClick={() => review()}>
                  <FileDocument />
                </TooltipIconButton>
              }
            >
              {files.map((file) => (
                <FileRow
                  key={file.path}
                  file={{ path: file.path, oldPath: file.from, status: file.status }}
                  onOpen={() => review(file.path)}
                  counts={!file.binary && <Counts insertions={file.insertions} deletions={file.deletions} className="opacity-70" />}
                />
              ))}
            </SourceGroup>
          )}
        </div>
      )}
    </div>
  );
};
