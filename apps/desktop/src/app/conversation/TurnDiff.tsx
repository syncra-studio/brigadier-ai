import { ArrowRotateCcw, ArrowRotateCw, ArrowUpRight } from "@openai/apps-sdk-ui/components/Icon";
import { type FC, useState } from "react";

import { isFinal } from "@/app/conversation/blocks";
import { useRequestDiff } from "@/app/conversation/ComposerCapsule";
import { useViewConversation } from "@/app/conversation/viewContext";
import { DiffGlyph } from "@/components/assistant-ui/elements/diff-glyph";
import { paper } from "@/components/assistant-ui/elements/surfaces";
import { Button } from "@/components/ui/button";
import { request } from "@/ipc/client";
import { cn } from "@/lib/utils";
import { useBoard } from "@/state/board";
import { setReviewScope } from "@/state/review";
import { openReviewTab } from "@/state/sessionTabs";
import { toast } from "@/state/toasts";

const Counts: FC<{ insertions: number; deletions: number; className?: string }> = ({
  insertions,
  deletions,
  className,
}) => (
  <span className={cn("tabular-nums", className)}>
    <span className="text-success">+{insertions}</span>{" "}
    <span className="text-destructive">−{deletions}</span>
  </span>
);

/**
 * The card under a finished turn's answer: "Edited N files +a −d", one row per file, and
 * Undo, which turns into Reapply. It sums what the request's workers landed, so it keeps its
 * counts in both states; a request that landed nothing has no card.
 */
export const TurnDiff: FC<{ requestId: string }> = ({ requestId }) => {
  const conversationId = useViewConversation()?.id ?? null;
  const diff = useRequestDiff(requestId);
  const reverted = useBoard((s) => s.board?.requests[requestId]?.undo?.reverted ?? false);
  // Undo waits for the request's workers: one still at work may land more.
  const working = useBoard((s) =>
    Object.values(s.board?.tasks ?? {}).some(
      (task) => task.requestId === requestId && !isFinal(task),
    ),
  );
  const [busy, setBusy] = useState(false);
  if (!diff || !conversationId) return null;

  const review = () => {
    setReviewScope(conversationId, { type: "lastTurn", requestId });
    openReviewTab(conversationId, { type: "all" });
  };

  const files = diff.files.length;
  const toggle = () => {
    setBusy(true);
    request({ method: "undoChanges", conversationId, requestId, reapply: reverted })
      .then(() => toast(reverted ? "Changes reapplied" : "Changes reverted"))
      .catch((error: unknown) =>
        toast(
          `Failed to revert changes: ${error instanceof Error ? error.message : String(error)}`,
          { tone: "error" },
        ),
      )
      .finally(() => setBusy(false));
  };

  return (
    <div data-slot="turn-diff" className={cn(paper, "rounded-xl overflow-hidden")}>
      <div className="group/diff flex items-center gap-3 p-2">
        <button
          type="button"
          aria-label="Review changed files"
          onClick={review}
          className="flex min-w-0 flex-1 items-center gap-3 text-start"
        >
          <span className="bg-foreground/5 text-muted-foreground rounded-control flex size-control-lg shrink-0 items-center justify-center">
            <DiffGlyph className="size-icon-md" />
          </span>
          <span className="flex min-w-0 flex-col">
            <span className="text-foreground text-sm">
              Edited {files} {files === 1 ? "file" : "files"}
            </span>
            <Counts
              insertions={diff.insertions}
              deletions={diff.deletions}
              className="text-xs group-hover/diff:hidden"
            />
            <span className="text-muted-foreground hidden items-center gap-0.5 text-xs group-hover/diff:flex">
              Review changes
              <ArrowUpRight className="size-icon-xs" />
            </span>
          </span>
        </button>
        <Button
          variant="ghost"
          size="sm"
          disabled={busy || working}
          onClick={toggle}
          className="text-muted-foreground hover:text-foreground font-normal"
        >
          {reverted ? "Reapply" : "Undo"}
          {reverted ? <ArrowRotateCw className="size-icon-sm" /> : <ArrowRotateCcw className="size-icon-sm" />}
        </Button>
        <Button variant="ghost" size="sm" onClick={review} className="border-border border font-normal">
          Review
        </Button>
      </div>
      <ul>
        {diff.files.map((file) => (
          <li
            key={file.path}
            className="border-border flex h-control-md items-center gap-3 border-t px-3 text-sm"
          >
            <span className="text-foreground min-w-0 flex-1 truncate" title={file.path}>
              {file.path}
            </span>
            {file.binary ? (
              <span className="text-muted-foreground text-xs">binary</span>
            ) : (
              <Counts insertions={file.insertions} deletions={file.deletions} className="text-xs" />
            )}
          </li>
        ))}
      </ul>
    </div>
  );
};
