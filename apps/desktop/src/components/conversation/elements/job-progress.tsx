import type { ComponentProps } from "react";

import { ghostButton, mono } from "@/components/conversation/elements/surfaces";
import { Check, ExclamationMarkCircle, Spinner, X } from "@/components/icons";
import { cn } from "@/lib/utils";

/**
 * assistant-ui's Long job element (https://www.assistant-ui.com/elements/job-progress, registry
 * item elements-job-progress) on Brigadier's tokens and icons, drawn as the user's JobCard
 * design: a line with the job, how long is left and a way out; a bar of one segment per stage
 * (done ones full, the one at work filled as far as it got, a failed one red); under it one line
 * naming the stage at work, shimmering, and the one after it, then that stage's description.
 */

export type JobStageState = "done" | "active" | "failed" | "pending";

export interface JobStage {
  name: string;
  weight: number;
  description?: string | undefined;
  /** Its own state; by default it follows `stageIndex` (done before it, pending after). */
  state?: JobStageState | undefined;
}

export type JobOutcomeStatus = "success" | "partial" | "failed" | "cancelled";

/** Constrains a value to `min…max`; NaN is `min`. */
function clamp(value: number, min: number, max: number) {
  if (Number.isNaN(value)) return min;
  return Math.min(max, Math.max(min, value));
}

/** `value` as a share of `total`, as a percentage in `0…100`. */
function pct(value: number, total: number) {
  if (!(total > 0)) return 0;
  return clamp((value / total) * 100, 0, 100);
}

/** A count of completed items out of `total`, in `0…total`. */
function progressOf(index: number, total: number) {
  if (!(total > 0)) return 0;
  return Math.floor(clamp(index, 0, total));
}

function formatElapsed(elapsedMs: number) {
  const seconds = Math.max(0, Math.round(Number.isFinite(elapsedMs) ? elapsedMs / 1_000 : 0));
  const minutes = Math.floor(seconds / 60);
  const remainder = seconds % 60;
  return {
    dateTime: `PT${seconds}S`,
    label: minutes === 0 ? `${seconds}s` : remainder === 0 ? `${minutes}m` : `${minutes}m ${remainder}s`,
  };
}

/** The part of the step where a job ended that it got through. */
const OUTCOME_PART: Record<JobOutcomeStatus, string> = {
  success: "bg-success",
  partial: "bg-warning",
  failed: "bg-destructive",
  cancelled: "bg-muted-foreground",
};

export function JobProgress({
  title,
  stages,
  stageIndex,
  stageProgress,
  eta,
  onCancel,
  outcome,
  elapsedMs,
  className,
  ...props
}: Omit<
  ComponentProps<"div">,
  "children" | "title" | "stages" | "stageIndex" | "stageProgress" | "eta" | "onCancel" | "outcome" | "elapsedMs"
> & {
  title: string;
  stages: readonly JobStage[];
  stageIndex: number;
  stageProgress: number;
  eta?: string | undefined;
  onCancel?: (() => void) | undefined;
  outcome?: { status: JobOutcomeStatus; summary?: string | undefined } | undefined;
  elapsedMs?: number | undefined;
}) {
  const stage = progressOf(stageIndex, stages.length);
  const progress = clamp(stageProgress, 0, 1);
  const totalWeight = stages.reduce((sum, item) => sum + item.weight, 0) || 1;
  const completed = stages.slice(0, stage).reduce((sum, item) => sum + item.weight, 0);
  const current = stages[stage];
  const overall = pct(completed + (current ? current.weight * progress : 0), totalWeight);
  const finished = stage >= stages.length;
  const running = !finished && outcome === undefined;
  const state = outcome?.status ?? (finished ? "done" : "running");
  const elapsed = !running && elapsedMs !== undefined ? formatElapsed(elapsedMs) : undefined;
  const statusWord = outcome?.status === "success" ? "done" : (outcome?.status ?? "done");
  const stateOf = (item: JobStage, index: number): JobStageState =>
    item.state ?? (index < stage || finished ? "done" : index === stage && running ? "active" : "pending");
  const failing = running && stages.some((item, index) => stateOf(item, index) === "failed");
  const announcement = outcome
    ? `Job ${outcome.status === "success" ? "done" : outcome.status === "partial" ? "partly done" : outcome.status}`
    : current
      ? `Current stage: ${current.name}`
      : finished
        ? "Job done"
        : "Job running";
  const progressPercent = outcome?.status === "success" ? 100 : overall;
  // The step the line under the bar names: the last one that failed, else the one at work or
  // where the job stopped; then the step after it. A job done in full names none.
  const failedAt = stages.findLastIndex((item, index) => stateOf(item, index) === "failed");
  const shownIndex = failedAt !== -1 ? failedAt : outcome?.status === "success" || finished ? -1 : stage;
  const shown = stages[shownIndex];
  const shownState = shown ? stateOf(shown, shownIndex) : "pending";
  const after = shown ? stages[shownIndex + 1] : undefined;
  // Each step's share of the bar: done ones full, a failed one red, the one at work filled as far
  // as it got (grey where the job was stopped), the rest an empty track; all green once done.
  const segmentFill = (at: JobStageState, index: number): { color: string; width: number } | null => {
    if (outcome?.status === "success" || (finished && !outcome)) return { color: "bg-success", width: 100 };
    if (at === "done") return { color: "bg-toggle-on", width: 100 };
    if (at === "failed") return { color: "bg-destructive", width: 100 };
    if (index !== stage || progress === 0) return null;
    if (at === "active") return { color: "bg-toggle-on", width: progress * 100 };
    if (outcome) return { color: OUTCOME_PART[outcome.status], width: progress * 100 };
    return null;
  };

  return (
    <div
      data-slot="job-progress"
      data-state={state}
      className={cn("flex w-full min-w-0 flex-col gap-3 text-sm", className)}
      {...props}
    >
      <div className="flex min-w-0 items-center gap-2.5">
        {outcome?.status === "partial" ? (
          <ExclamationMarkCircle aria-hidden className="text-warning size-icon-sm shrink-0" />
        ) : outcome?.status === "failed" ? (
          <X aria-hidden className="text-destructive size-icon-sm shrink-0" />
        ) : outcome?.status === "cancelled" ? (
          <X aria-hidden className="text-muted-foreground size-icon-sm shrink-0" />
        ) : !running ? (
          <Check aria-hidden className="text-success size-icon-sm shrink-0" />
        ) : (
          <Spinner
            className={cn(
              "size-icon-sm shrink-0 motion-safe:animate-spin",
              failing ? "text-destructive" : "text-muted-foreground",
            )}
          />
        )}
        <span className={cn("min-w-0 flex-1 truncate font-medium", failing && "text-destructive")}>{title}</span>
        {(elapsed || !running || eta) && (
          <span className={cn(mono, "text-muted-foreground shrink-0 tabular-nums")}>
            {elapsed ? <time dateTime={elapsed.dateTime}>{elapsed.label}</time> : !running ? statusWord : eta}
          </span>
        )}
        {running && onCancel ? (
          <button type="button" aria-label="Cancel the job" onClick={onCancel} className={cn(ghostButton, "size-control-xs")}>
            <X className="size-icon-sm" />
          </button>
        ) : null}
      </div>

      <span
        role="progressbar"
        aria-label={`${title} progress`}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(progressPercent * 10) / 10}
        className="flex w-full gap-0.75"
      >
        {stages.map((item, index) => {
          const at = stateOf(item, index);
          const fill = segmentFill(at, index);
          return (
            <span
              key={`${item.name}-${index}`}
              data-segment={at}
              className="bg-foreground/10 h-1 min-w-0 basis-0 overflow-hidden rounded-full"
              style={{ flexGrow: item.weight }}
            >
              {fill && (
                <span
                  className={cn(
                    "block h-full rounded-full motion-safe:transition-[width] motion-safe:duration-500 motion-safe:ease-out",
                    fill.color,
                  )}
                  style={{ width: `${fill.width}%` }}
                />
              )}
            </span>
          );
        })}
      </span>

      {shown && (
        <div className="flex min-w-0 flex-col gap-0.5">
          <p className={cn(mono, "flex min-w-0 items-baseline gap-1.5")}>
            <span
              data-stage={shownState}
              className={cn(
                "min-w-0 shrink truncate",
                shownState === "failed"
                  ? "text-destructive"
                  : shownState === "active"
                    ? "shimmer motion-reduce:animate-none"
                    : "text-muted-foreground",
              )}
            >
              {shown.name}
            </span>
            {after && (
              <>
                <span aria-hidden className="text-muted-foreground/40 shrink-0">
                  →
                </span>
                <span data-stage="next" className="text-muted-foreground/60 min-w-0 truncate">
                  {after.name}
                </span>
              </>
            )}
          </p>
          {shown.description ? <p className="text-muted-foreground text-xs break-words">{shown.description}</p> : null}
        </div>
      )}
      {outcome?.summary ? <p className="text-sm break-words">{outcome.summary}</p> : null}
      <span className="sr-only" role="status">
        {announcement}
      </span>
    </div>
  );
}
