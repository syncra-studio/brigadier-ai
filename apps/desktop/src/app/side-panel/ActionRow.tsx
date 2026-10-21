import type { ReactNode } from "react";

import { StepRow } from "@/app/conversation/activity/ActivityGroup";
import { changeStats, FileChangeRows } from "@/app/conversation/activity/EditedFiles";
import { InlineDiff } from "@/app/conversation/activity/InlineDiff";
import { actionDetail } from "@/app/conversation/activity/StepDetail";
import { type ActionItem, itemCall, stepWords } from "@/app/conversation/activity/words";
import { formatDuration } from "@/lib/format";

/** A worker's command's exit code, when it says. */
export const exitOf = (item: ActionItem): number | null => (item.kind === "command" ? item.exitCode : null);

/** A worker's action, as the lead's same step would read; a command or call opens to what it ran. */
export function ActionRow({ item }: { item: ActionItem }) {
  // A file change is a row per file, as the thread's own is.
  if (item.kind === "files" && item.changes.length > 0) {
    const diffs = new Map<string, ReactNode>();
    for (const change of item.changes) {
      const path = change.movedTo ?? change.path;
      if (change.diff) diffs.set(path, <InlineDiff key={path} path={path} diff={change.diff} />);
    }
    return <FileChangeRows files={changeStats(item.changes)} status={item.status} diffs={diffs} />;
  }
  const call = itemCall(item);
  const ran = item.kind === "command" && item.status !== "inProgress" && item.durationMs !== null && item.durationMs >= 1000;
  return (
    <StepRow
      words={stepWords(call)}
      status={call.status}
      exit={exitOf(item)}
      detail={actionDetail(item)}
      suffix={ran && item.durationMs !== null ? `in ${formatDuration(item.durationMs)}` : undefined}
    />
  );
}
