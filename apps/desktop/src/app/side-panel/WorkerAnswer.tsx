import { useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { useWorkerText, WorkerLine } from "@/app/conversation/WorkerChip";
import { ArtifactDialog } from "@/app/side-panel/ArtifactDialog";
import { FileList } from "@/app/side-panel/FileList";
import { MarkdownBlock, RateItem, RateMenu } from "@/components/conversation";
import { TooltipIconButton } from "@/components/controls";
import { Check, Copy } from "@/components/icons";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { useNow } from "@/hooks/use-now";
import type { ArtifactRef, Rating, Task } from "@/ipc/generated";
import { formatSentAt } from "@/lib/format";
import { rateMessage } from "@/state/actions";
import { useBoard } from "@/state/board";

/**
 * The report as the thread's answer, with copy and rate; when it came shows on hover. `earlier`:
 * the worker was sent back to work after it, so it is history, not the answer to open at.
 */
export function WorkerAnswer({ task, text, earlier = false }: { task: Task; text?: string | undefined; earlier?: boolean }) {
  const { isCopied, copyToClipboard } = useCopyToClipboard();
  const now = useNow(60_000);
  const subject = `task:${task.id}`;
  const rated = useBoard((s) => s.board?.ratings[subject] ?? null);
  const action = useAction();
  const summary = useWorkerText(text ?? task.report?.summary ?? "");
  const [artifact, setArtifact] = useState<ArtifactRef | null>(null);
  const files = [...new Map([...(task.report?.artifacts ?? []), ...task.outputs].map((file) => [file.id, file])).values()];
  if (!summary) return null;
  const rate = (rating: Rating | null) =>
    action.run(() => rateMessage(task.conversationId, subject, rating));
  return (
    <div data-slot={earlier ? "worker-earlier-report" : "worker-report"} className="group/answer flex scroll-mt-4 flex-col gap-2">
      <div className="text-foreground min-w-0 leading-relaxed wrap-anywhere">
        <MarkdownBlock text={summary} />
      </div>
      {files.length > 0 && <FileList artifacts={files} onView={setArtifact} />}
      {task.report?.needsUser.map((need, index) => <p key={index} className="leading-relaxed"><WorkerLine text={need} /></p>)}
      <ArtifactDialog artifact={artifact} onOpenChange={(next) => !next && setArtifact(null)} />
      <div className="text-muted-foreground -ms-1 flex items-center gap-1">
        <TooltipIconButton
          tooltip={isCopied ? "Copied" : "Copy"}
          onClick={() => copyToClipboard(summary)}
        >
          {isCopied ? <Check /> : <Copy />}
        </TooltipIconButton>
        <RateMenu rated={rated} onClear={() => rate(null)}>
          <RateItem rating="good" onSelect={() => rate("good")} />
          <RateItem rating="bad" onSelect={() => rate("bad")} />
        </RateMenu>
        <span className="ps-1 text-xs tabular-nums opacity-0 transition-opacity group-hover/answer:opacity-100">
          {formatSentAt(task.updatedAtMs, now)}
        </span>
        {action.error && (
          <span role="alert" className="text-destructive ps-1 text-xs">
            {action.error}
          </span>
        )}
      </div>
    </div>
  );
}
