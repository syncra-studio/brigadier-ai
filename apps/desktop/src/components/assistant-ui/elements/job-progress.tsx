import type { ComponentProps } from "react";

import { ghostButton, mono } from "@/components/assistant-ui/elements/surfaces";
import { Check, ExclamationMarkCircle, Spinner, X } from "@/components/icons";
import { cn } from "@/lib/utils";

/**
 * assistant-ui's Long job element (https://www.assistant-ui.com/elements/job-progress, registry
 * item elements-job-progress), ported as is onto Brigadier's tokens and icons: a line with the
 * job, how long is left and a way out, one bar filled by the weighted stages done, and the
 * stages listed under it with the one at work's description. For the Job card each stage also
 * carries its mark: a check once it passed, a dim check while it is still to come, the stage at
 * work shimmering, a cross on one that failed.
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

const OUTCOME_BAR: Record<JobOutcomeStatus, string> = {
  success: "bg-success",
  partial: "bg-warning",
  failed: "bg-destructive",
  cancelled: "bg-foreground/20",
};

/** A stage's mark in the list. */
function StageMark({ state }: { state: JobStageState }) {
  if (state === "failed") return <X aria-hidden className="text-destructive size-icon-xs shrink-0" />;
  return (
    <Check
      aria-hidden
      data-mark={state}
      className={cn("size-icon-xs shrink-0", state === "done" ? "text-success" : "text-muted-foreground/60")}
    />
  );
}

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
  const outcomeBar = outcome ? OUTCOME_BAR[outcome.status] : undefined;
  const progressPercent = outcome?.status === "success" ? 100 : overall;

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
        className="bg-foreground/10 h-1 w-full overflow-hidden rounded-full"
      >
        <span
          className={cn(
            "block h-full rounded-full motion-safe:transition-[width] motion-safe:duration-500 motion-safe:ease-out",
            outcomeBar ?? (finished ? "bg-success" : failing ? "bg-destructive" : "bg-link"),
          )}
          style={{ width: `${progressPercent}%` }}
        />
      </span>

      <div className="flex flex-col gap-0.5">
        <div className="flex flex-wrap gap-x-3 gap-y-1">
          {stages.map((item, index) => {
            const at = stateOf(item, index);
            return (
              <span
                key={`${item.name}-${index}`}
                data-stage={at}
                className={cn(
                  mono,
                  "inline-flex items-center gap-1",
                  at === "failed" ? "text-destructive" : at === "active" ? "text-foreground" : "text-muted-foreground",
                )}
              >
                <StageMark state={at} />
                <span className={cn(at === "active" && "shimmer shimmer-step motion-reduce:animate-none")}>{item.name}</span>
              </span>
            );
          })}
        </div>
        {current?.description ? (
          <p className="text-muted-foreground text-xs break-words">{current.description}</p>
        ) : null}
      </div>
      {outcome?.summary ? <p className="text-sm break-words">{outcome.summary}</p> : null}
      <span className="sr-only" role="status">
        {announcement}
      </span>
    </div>
  );
}
