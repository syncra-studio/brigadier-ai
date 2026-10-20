import type { ComponentProps } from "react";

import { mono } from "@/components/conversation/elements/surfaces";
import { Check, Clock, X } from "@/components/icons";
import { formatCountdown, formatTime } from "@/lib/format";
import { cn } from "@/lib/utils";

/**
 * What happens next at a set time (assistant-ui's schedule card element,
 * https://www.assistant-ui.com/elements, on Brigadier's tokens and icons): what will happen and
 * why at that time, how long until then, and the earlier tries when it is a retry. No pause
 * switch: nothing here is a setting.
 */

export interface ScheduleRun {
  id: string;
  atMs: number;
  ok: boolean;
}

export function ScheduleCard({
  title,
  reason,
  atMs,
  nowMs,
  history,
  className,
  ...props
}: Omit<ComponentProps<"div">, "children" | "title"> & {
  /** "Resumes at 4:00 AM". */
  title: string;
  /** "when the Claude limit resets". */
  reason?: string | undefined;
  /** When it happens. */
  atMs: number;
  nowMs: number;
  /** Earlier tries, oldest first. */
  history?: readonly ScheduleRun[] | undefined;
}) {
  const left = formatCountdown(atMs, nowMs);
  return (
    <div data-slot="schedule-card" className={cn("flex w-full min-w-0 gap-2", className)} {...props}>
      {/* The icon on the first line's height; everything else lines up after it. */}
      <span className="flex h-(--text-label--line-height) shrink-0 items-center">
        <Clock aria-hidden className="text-muted-foreground size-icon-sm" />
      </span>
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <div className="flex min-w-0 items-center gap-2">
          <span className="min-w-0 flex-1 truncate">{title}</span>
          <time
            dateTime={new Date(atMs).toISOString()}
            title={formatTime(atMs)}
            className={cn(mono, "text-muted-foreground shrink-0 tabular-nums")}
          >
            {left === "now" ? "now" : `in ${left}`}
          </time>
        </div>
        {reason && <p className="text-muted-foreground text-xs wrap-anywhere">{reason}</p>}
        {history && history.length > 0 && (
          <ul className="flex flex-col gap-0.5 pt-1">
            {history.map((run) => (
              <li key={run.id} className="text-muted-foreground flex min-w-0 items-center gap-2 text-xs">
                {run.ok ? (
                  <Check aria-hidden className="size-icon-xs shrink-0" />
                ) : (
                  <X aria-hidden className="text-destructive size-icon-xs shrink-0" />
                )}
                <span className="min-w-0 flex-1 truncate">{formatTime(run.atMs)}</span>
                <span className={cn(run.ok ? "" : "text-destructive")}>{run.ok ? "Done" : "Failed"}</span>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
