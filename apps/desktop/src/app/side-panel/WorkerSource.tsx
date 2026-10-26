import { type FC, type ReactNode, useCallback, useEffect, useState } from "react";

import { useWorkerName } from "@/app/conversation/WorkerChip";
import { FileRow, SourcePanel } from "@/app/side-panel/SourcePanel";
import { SourceGroup } from "@/app/side-panel/SourceGroup";
import { SourceHeader } from "@/app/side-panel/SourceHeader";
import { reason, setSourceScope, useSourceScopes } from "@/app/side-panel/sourceTarget";
import { WorkerGlyph } from "@/components/conversation";
import { TooltipIconButton } from "@/components/controls";
import { Branch, ChevronDown, FileDocument } from "@/components/icons";
import { DropdownMenu, DropdownMenuContent, DropdownMenuRadioGroup, DropdownMenuRadioItem, DropdownMenuTrigger } from "@/components/overlays";
import { Counts } from "@/components/shared";
import { request, RequestError } from "@/ipc/client";
import type { WorkerRow, WorkerSource } from "@/ipc/generated";
import { useWorkerRows } from "@/state/cliThread";
import { openReviewTab } from "@/state/sessionTabs";

/**
 * A session's Source: its own checkout, as the Source panel shows it, or, picked in the switch
 * under the title (or from a worker's +N −N in the context card), one of its workers' changes.
 * A worker's are read only: it writes there.
 */
export function SessionSource({ conversationId }: { conversationId: string }) {
  const picked = useSourceScopes((scopes) => scopes[conversationId] ?? null);
  // The workers whose worktree git reads: their +N −N is what the worker scope lists.
  const all = useWorkerRows(conversationId);
  const workers = all.filter((row) => row.diff !== null);
  const worker = workers.find((row) => row.id === picked) ?? null;
  // A worker that left the list (closed, its worktree gone) leaves the session's scope in its place.
  const stale = picked !== null && worker === null;
  useEffect(() => {
    if (stale) setSourceScope(conversationId, null);
  }, [conversationId, stale]);
  const scopeSwitch = workers.length > 0 && (
    <SourceScopeSwitch conversationId={conversationId} workers={workers} shown={worker?.id ?? null} />
  );
  return worker
    ? <WorkerSourcePanel key={worker.id} conversationId={conversationId} worker={worker} scopeSwitch={scopeSwitch} />
    : <SourcePanel target={{ type: "conversation", conversationId }} scopeSwitch={scopeSwitch} />;
}

const SESSION = "session";

/** Under Source's title: whose changes it shows, the session's or a worker's, picked from a menu. */
const SourceScopeSwitch: FC<{ conversationId: string; workers: readonly WorkerRow[]; shown: string | null }> = ({
  conversationId,
  workers,
  shown,
}) => (
  <div className="flex shrink-0 px-2 pb-1">
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          data-slot="source-scope"
          aria-label="Show changes of"
          className="hover:bg-muted rounded-control text-muted-foreground flex h-control-sm min-w-0 flex-1 items-center gap-1.5 px-1 text-sm"
        >
          {shown ? <WorkerGlyph taskId={shown} className="size-icon-sm shrink-0" /> : <Branch className="size-icon-sm shrink-0" />}
          <span className="text-foreground min-w-0 flex-1 truncate text-start">{shown ? <WorkerLabel taskId={shown} /> : "This session"}</span>
          <ChevronDown className="size-icon-sm shrink-0" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="min-w-(--radix-dropdown-menu-trigger-width)">
        <DropdownMenuRadioGroup value={shown ?? SESSION} onValueChange={(value) => setSourceScope(conversationId, value === SESSION ? null : value)}>
          <DropdownMenuRadioItem value={SESSION}>
            <Branch className="size-icon-sm shrink-0" />
            This session
          </DropdownMenuRadioItem>
          {workers.map((row) => (
            <DropdownMenuRadioItem key={row.id} value={row.id}>
              <WorkerGlyph taskId={row.id} className="size-icon-sm shrink-0" />
              <span className="min-w-0 flex-1 truncate"><WorkerLabel taskId={row.id} /></span>
              {row.diff && <Counts insertions={row.diff.insertions} deletions={row.diff.deletions} />}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  </div>
);

const WorkerLabel: FC<{ taskId: string }> = ({ taskId }) => <>{useWorkerName(taskId) ?? "Worker"}</>;

/**
 * A worker's changes, read again when its +N −N moves, on focus and on Refresh: its branch since
 * it left the session's, committed and not.
 */
function useWorkerSource(conversationId: string, worker: WorkerRow) {
  const [source, setSource] = useState<WorkerSource | null>(null);
  const [error, setError] = useState<string | null>(null);
  const taskId = worker.id;
  const load = useCallback(() => {
    request({ method: "getWorkerSource", conversationId, taskId })
      .then(({ source: next }) => {
        setSource(next);
        setError(null);
      })
      .catch((cause: unknown) => {
        // Gone (closed, its worktree removed): the session's own scope shows instead, quietly.
        if (cause instanceof RequestError && cause.code === "notFound") setSourceScope(conversationId, null);
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
  return { source, error, load };
}

/**
 * Source on a worker: its branch, and the files it changed with their letters and +N −N, each
 * opening its part of the worker's diff in the Review tab. Nothing is staged, committed or
 * discarded here.
 */
const WorkerSourcePanel: FC<{ conversationId: string; worker: WorkerRow; scopeSwitch: ReactNode }> = ({
  conversationId,
  worker,
  scopeSwitch,
}) => {
  const { source, error, load } = useWorkerSource(conversationId, worker);
  const [open, setOpen] = useState(true);
  const files = source?.files ?? [];
  const review = (path?: string) =>
    openReviewTab(conversationId, { type: "worker", taskId: worker.id, ...(path ? { path } : {}), staged: false });
  return (
    <div data-slot="source-panel" data-scope="worker" className="relative flex min-h-0 flex-1 flex-col">
      <SourceHeader open={open} onToggle={() => setOpen(!open)} readOnly locked={false} onCommit={() => {}} onRefresh={load} moreMenu={null} />
      {scopeSwitch}
      <div className={open ? "flex min-h-0 flex-1 flex-col" : "hidden"}>
        {source && (
          <div data-slot="source-branch" className="text-muted-foreground flex h-control-sm shrink-0 items-center gap-1.5 px-3 text-sm">
            <Branch className="size-icon-sm shrink-0" />
            <span className="text-foreground min-w-0 truncate">{source.branch ?? "…"}</span>
            <span className="min-w-0 shrink-0 truncate text-xs">into {source.base}</span>
          </div>
        )}
        <div className="min-h-0 flex-1 overflow-y-auto px-2 pb-2">
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
      </div>
    </div>
  );
};
