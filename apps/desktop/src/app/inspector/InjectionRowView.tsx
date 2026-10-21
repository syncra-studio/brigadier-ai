import { memo } from "react";

import {
  formatTokens,
  GROUPS,
  groupOf,
  type InjectionRow,
  KIND_LABELS,
} from "@/app/inspector/orchestratorLog";
import { formatBytes, formatClock } from "@/lib/format";
import { cn } from "@/lib/utils";

const SWATCHES = Object.fromEntries(GROUPS.map((group) => [group.id, group.swatch]));

export const InjectionRowView = memo(function InjectionRowView({
  row,
  taskNumber,
}: {
  row: InjectionRow;
  taskNumber: number | null;
}) {
  const { injection } = row;
  return (
    <div className="h-row-sm flex items-center gap-2 px-3 font-mono text-xs">
      <span className="text-muted-foreground shrink-0 tabular-nums">{formatClock(row.atMs)}</span>
      <span
        aria-hidden
        className={cn("size-2 shrink-0 rounded-xs", SWATCHES[groupOf(injection.kind)])}
      />
      <span className="text-foreground shrink-0">{KIND_LABELS[injection.kind]}</span>
      <span className="text-muted-foreground min-w-0 flex-1 truncate" title={injection.label}>
        {injection.label}
        {taskNumber !== null && ` · task-${taskNumber}`}
      </span>
      <span className="shrink-0 tabular-nums">{formatTokens(injection.tokensEstimate)} tok</span>
      <span className="text-muted-foreground w-16 shrink-0 text-end tabular-nums">
        {formatBytes(injection.bytes)}
      </span>
    </div>
  );
});
