import { useState } from "react";

import { formatTokens, type RebirthRow } from "@/app/inspector/orchestratorLog";
import { RebirthDetail } from "@/app/inspector/RebirthDetail";
import { rebirthTimes, tookText } from "@/app/inspector/rebirthTimes";
import { ChevronRight } from "@/components/icons";
import { Badge } from "@/components/controls";
import type { RebirthRecord } from "@/ipc/generated";
import { formatClock } from "@/lib/format";
import { cn } from "@/lib/utils";

const TRIGGERS: Record<RebirthRecord["trigger"], string> = {
  threshold: "threshold",
  recovery: "recovery",
  cacheExpired: "cache expired",
  midTurn: "stopped mid-turn",
};

export function RebirthRowView({ row }: { row: RebirthRow }) {
  const [open, setOpen] = useState(false);
  const { record } = row;
  return (
    <li className="border-b last:border-b-0">
      <button
        type="button"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
        className="hover:bg-accent/50 flex w-full flex-col gap-0.5 px-3 py-2 text-start"
      >
        <span className="flex items-center gap-2">
          <ChevronRight
            aria-hidden
            className={cn("text-muted-foreground size-icon-xs shrink-0 transition-transform", open && "rotate-90")}
          />
          <span className="font-medium">Generation {record.generation}</span>
          <Badge variant={record.trigger === "threshold" ? "secondary" : "warning"}>
            {TRIGGERS[record.trigger]}
          </Badge>
          <span className="flex-1" />
          <span className="text-muted-foreground tabular-nums">{formatClock(row.atMs)}</span>
        </span>
        <span className="text-muted-foreground ps-5 tabular-nums">
          at {formatTokens(record.atTokens)} tokens · briefing {formatTokens(record.briefingTokens)} ·{" "}
          {record.decisions} decisions ({record.decisionsInFull} in full) · {record.recentMessages} recent
          messages · {tookText(rebirthTimes(record))}
        </span>
      </button>
      {open && <RebirthDetail record={record} />}
    </li>
  );
}
