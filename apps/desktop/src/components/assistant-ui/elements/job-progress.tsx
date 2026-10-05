import { Check, ExclamationMarkCircle, X } from "@openai/apps-sdk-ui/components/Icon";
import type { ComponentProps, ReactNode } from "react";

import { mono } from "@/components/assistant-ui/elements/surfaces";
import { Spinner } from "@/components/glyphs/spinner";
import { cn } from "@/lib/utils";

/**
 * One job's progress through stages that run one after another (assistant-ui's job progress
 * element, https://www.assistant-ui.com/elements, on Brigadier's tokens and icons): a line with
 * the stage at work and how long is left, a bar, and once it ends, a receipt in its place.
 */

export type JobOutcomeStatus = "success" | "partial" | "failed" | "cancelled";

/** `value` as a share of `total`, as a percentage in `0…100`. */
function pct(value: number, total: number): number {
  if (!(total > 0) || Number.isNaN(value)) return 0;
  return Math.min(100, Math.max(0, (value / total) * 100));
}

const OUTCOME_BAR: Record<JobOutcomeStatus, string> = {
  success: "bg-success",
  partial: "bg-warning",
  failed: "bg-destructive",
  cancelled: "bg-foreground/20",
};

export function JobProgress({
  title,
  done,
  total,
  meta,
  description,
  outcome,
  className,
  ...props
}: Omit<ComponentProps<"div">, "children" | "title"> & {
  /** The stage at work ("Phase 2 of 3 · Fix"), or the job once it ended. */
  title: string;
  /** Stages finished, of `total`. */
  done: number;
  total: number;
  /** At the line's end: what is left while it runs ("until 07:30"), how long it took after. */
  meta?: ReactNode;
  /** What the stage at work is doing, in a line. */
  description?: ReactNode;
  outcome?: { status: JobOutcomeStatus; summary?: string | undefined } | undefined;
}) {
  const running = outcome === undefined;
  const progress = outcome?.status === "success" ? 100 : pct(done, total);
  return (
    <div
      data-slot="job-progress"
      data-state={outcome?.status ?? "running"}
      className={cn("flex w-full min-w-0 flex-col gap-2", className)}
      {...props}
    >
      <div className="flex min-w-0 items-center gap-2">
        {outcome?.status === "partial" ? (
          <ExclamationMarkCircle aria-hidden className="text-warning size-icon-sm shrink-0" />
        ) : outcome?.status === "failed" ? (
          <X aria-hidden className="text-destructive size-icon-sm shrink-0" />
        ) : outcome?.status === "cancelled" ? (
          <X aria-hidden className="text-muted-foreground size-icon-sm shrink-0" />
        ) : outcome ? (
          <Check aria-hidden className="text-success size-icon-sm shrink-0" />
        ) : (
          <Spinner className="text-muted-foreground size-icon-sm shrink-0 motion-safe:animate-spin" />
        )}
        <span className={cn("min-w-0 flex-1 truncate text-sm", running && "shimmer")}>{title}</span>
        {meta && <span className={cn(mono, "text-muted-foreground shrink-0 tabular-nums")}>{meta}</span>}
      </div>
      <span
        role="progressbar"
        aria-label={`${title} progress`}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(progress)}
        className="bg-foreground/10 h-0.5 w-full overflow-hidden rounded-full"
      >
        <span
          className={cn(
            "block h-full rounded-full motion-safe:transition-[width] motion-safe:duration-500",
            outcome ? OUTCOME_BAR[outcome.status] : "bg-foreground/80",
          )}
          style={{ width: `${progress}%` }}
        />
      </span>
      {description && <div className="text-muted-foreground text-xs wrap-anywhere">{description}</div>}
      {outcome?.summary && <p className="text-sm wrap-anywhere">{outcome.summary}</p>}
    </div>
  );
}
