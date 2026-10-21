import type { ComponentProps, ReactNode } from "react";

import { ghostButton, mono } from "@/components/conversation/elements/surfaces";
import { Branch, Check, ExclamationMarkCircle, HandRaised, Moon, Spinner, Stop, StopCircleFilled, X } from "@/components/icons";
import { cn } from "@/lib/utils";

/**
 * assistant-ui's Long job element (https://www.assistant-ui.com/elements/job-progress, registry
 * item elements-job-progress) on Brigadier's tokens and icons, drawn as the user's JobCard
 * design: a line with the job, how long is left and a way out; a bar of one segment per stage
 * (done ones full, the one at work filled as far as it got, a failed one red); under it one line
 * naming the stage at work, shimmering, and the one after it, then that stage's description and
 * the job's own line. A proposed job (not started) lists its stages one per line instead, with
 * what it asks of the user; a job's actions sit last, as buttons of their own.
 */

/** `partial`: it ended with part of its work done. */
export type JobStageState = "done" | "active" | "failed" | "partial" | "pending";

export interface JobStage {
  name: string;
  weight: number;
  description?: string | undefined;
  /** Its own state; by default it follows `stageIndex` (done before it, pending after). */
  state?: JobStageState | undefined;
}

export type JobOutcomeStatus = "success" | "partial" | "failed" | "cancelled";

/**
 * How a live job reads now, whatever its stages: at work (a spinner), waiting on the user (a
 * raised hand), failed (a red cross), done (a tick), stopped, or merged. With it the card says
 * what is happening; a bar shows only when it has stages.
 */
export type JobTone = "running" | "waiting" | "failed" | "done" | "stopped" | "merged";

/** Each tone's icon. */
function ToneIcon({ tone }: { tone: JobTone }) {
  switch (tone) {
    case "running":
      return <Spinner className="text-muted-foreground size-icon-sm shrink-0 motion-safe:animate-spin" />;
    case "waiting":
      return <HandRaised aria-hidden className="text-warning size-icon-sm shrink-0" />;
    case "failed":
      return <X aria-hidden className="text-destructive size-icon-sm shrink-0" />;
    case "done":
      return <Check aria-hidden className="text-success size-icon-sm shrink-0" />;
    case "stopped":
      return <StopCircleFilled aria-hidden className="text-muted-foreground size-icon-sm shrink-0" />;
    case "merged":
      return <Branch aria-hidden className="text-success size-icon-sm shrink-0" />;
  }
}

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
  const hours = Math.floor(minutes / 60);
  return {
    dateTime: `PT${seconds}S`,
    label:
      hours > 0
        ? `${hours}h ${String(minutes % 60).padStart(2, "0")}m`
        : minutes === 0
          ? `${seconds}s`
          : remainder === 0
            ? `${minutes}m`
            : `${minutes}m ${remainder}s`,
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
  proposed = false,
  meta,
  detail,
  warning,
  actions,
  stalled = false,
  tone,
  status,
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
  /** Not started yet: its stages listed, nothing at work. */
  proposed?: boolean | undefined;
  /** Mono words on the right of the line, e.g. "3 phases". */
  meta?: string | undefined;
  /** The job's own line under its steps or its outcome: "Wraps up at 7:30". */
  detail?: string | undefined;
  /** What the user should know before a proposed job starts, in one muted line. */
  warning?: string | undefined;
  /** Its buttons, last. */
  actions?: ReactNode;
  /** No stage is at work: after a failure the line shows it still, with no spinner. */
  stalled?: boolean | undefined;
  /** A live job's state, apart from its stages: it picks the icon, and the timer shows while it runs. */
  tone?: JobTone | undefined;
  /** What a screen reader hears of it: by default, the stage at work or how it ended. */
  status?: string | undefined;
}) {
  const stage = progressOf(stageIndex, stages.length);
  const progress = clamp(stageProgress, 0, 1);
  const totalWeight = stages.reduce((sum, item) => sum + item.weight, 0) || 1;
  const completed = stages.slice(0, stage).reduce((sum, item) => sum + item.weight, 0);
  const current = stages[stage];
  const overall = proposed ? 0 : pct(completed + (current ? current.weight * progress : 0), totalWeight);
  // A live job is over only when its tone says so, never for want of stages.
  const finished = !proposed && !tone && stage >= stages.length;
  const running = !finished && outcome === undefined;
  const state = tone ?? (proposed ? "proposed" : (outcome?.status ?? (finished ? "done" : "running")));
  const elapsed = (tone || !running) && elapsedMs !== undefined ? formatElapsed(elapsedMs) : undefined;
  const statusWord = outcome?.status === "success" ? "done" : (outcome?.status ?? "done");
  const stateOf = (item: JobStage, index: number): JobStageState =>
    item.state ??
    (proposed ? "pending" : index < stage || finished ? "done" : index === stage && running ? "active" : "pending");
  const failing = running && stages.some((item, index) => stateOf(item, index) === "failed");
  const announcement = status ?? (proposed
    ? "Job not started"
    : outcome
      ? `Job ${outcome.status === "success" ? "done" : outcome.status === "partial" ? "partly done" : outcome.status}`
      : current
        ? `Current stage: ${current.name}`
        : finished
          ? "Job done"
          : "Job running");
  const progressPercent = outcome?.status === "success" ? 100 : overall;
  // The step the line under the bar names: the last one that failed, else the one at work or
  // where the job stopped; then the step after it. A job done in full names none.
  const failedAt = stages.findLastIndex((item, index) => stateOf(item, index) === "failed");
  const shownIndex = proposed ? -1 : failedAt !== -1 ? failedAt : outcome?.status === "success" || finished ? -1 : stage;
  const shown = stages[shownIndex];
  const shownState = shown ? stateOf(shown, shownIndex) : "pending";
  const after = shown ? stages[shownIndex + 1] : undefined;
  // Each step's share of the bar: done ones full, a failed one red, the one at work filled as far
  // as it got (grey where the job was stopped), the rest an empty track; all green once done.
  const segmentFill = (at: JobStageState, index: number): { color: string; width: number } | null => {
    if (proposed) return null;
    if (outcome?.status === "success" || (finished && !outcome)) return { color: "bg-success", width: 100 };
    if (at === "done") return { color: "bg-toggle-on", width: 100 };
    if (at === "failed") return { color: "bg-destructive", width: 100 };
    if (at === "partial") return { color: "bg-warning", width: 100 };
    if (index !== stage || progress === 0) return null;
    if (at === "active") return { color: "bg-toggle-on", width: progress * 100 };
    if (outcome) return { color: OUTCOME_PART[outcome.status], width: progress * 100 };
    return null;
  };
  const right = elapsed ? <time dateTime={elapsed.dateTime}>{elapsed.label}</time> : tone ? (meta ?? eta) : !running ? statusWord : (meta ?? eta);
  const lines = [shown?.description, outcome?.summary ? undefined : detail].filter(Boolean);

  return (
    <div
      data-slot="job-progress"
      data-state={state}
      className={cn("flex w-full min-w-0 flex-col gap-3 text-sm", className)}
      {...props}
    >
      <div className="flex min-w-0 items-center gap-2.5">
        {tone ? (
          <ToneIcon tone={tone} />
        ) : proposed ? (
          <Moon aria-hidden className="text-muted-foreground size-icon-sm shrink-0" />
        ) : outcome?.status === "partial" ? (
          <ExclamationMarkCircle aria-hidden className="text-warning size-icon-sm shrink-0" />
        ) : outcome?.status === "failed" ? (
          <X aria-hidden className="text-destructive size-icon-sm shrink-0" />
        ) : outcome?.status === "cancelled" ? (
          <X aria-hidden className="text-muted-foreground size-icon-sm shrink-0" />
        ) : !running ? (
          <Check aria-hidden className="text-success size-icon-sm shrink-0" />
        ) : stalled && failing ? (
          <X aria-hidden className="text-destructive size-icon-sm shrink-0" />
        ) : (
          <Spinner
            className={cn(
              "size-icon-sm shrink-0 motion-safe:animate-spin",
              failing ? "text-destructive" : "text-muted-foreground",
            )}
          />
        )}
        <span
          className={cn(
            "min-w-0 flex-1 truncate font-medium",
            (tone ? tone === "failed" : failing || outcome?.status === "failed") && "text-destructive",
          )}
        >
          {title}
        </span>
        {right && <span className={cn(mono, "text-muted-foreground shrink-0 tabular-nums")}>{right}</span>}
        {running && !proposed && onCancel ? (
          <button
            type="button"
            aria-label="Stop"
            title="Stop"
            onClick={onCancel}
            className={cn(ghostButton, "rounded-toolbar-button size-icon-button-xs pointer-events-auto")}
          >
            <Stop className="size-icon-xs" />
          </button>
        ) : null}
      </div>

      {stages.length > 0 && (
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
      )}

      {(proposed || shown || outcome?.summary || lines.length > 0 || actions) && (
        <div className="flex min-w-0 flex-col gap-2.5">
          {proposed ? (
            <ul className="flex min-w-0 flex-col gap-1">
              {stages.map((item, index) => (
                <li key={`${item.name}-${index}`} data-stage="pending" className={cn(mono, "text-muted-foreground flex min-w-0 items-center gap-1.5")}>
                  <Check aria-hidden className="text-muted-foreground/60 size-icon-xs shrink-0" />
                  <span className="min-w-0 truncate">{item.name}</span>
                </li>
              ))}
            </ul>
          ) : (
            (shown || lines.length > 0) && (
              <div className="flex min-w-0 flex-col gap-0.5">
                {shown && (
                  <p className={cn(mono, "flex min-w-0 items-baseline gap-1.5")}>
                    <span
                      data-stage={shownState}
                      className={cn(
                        "min-w-0 shrink truncate",
                        shownState === "failed"
                          ? "text-destructive"
                          : shownState === "partial"
                            ? "text-warning"
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
                )}
                {lines.map((line) => (
                  <p key={line} data-slot="job-detail" className="text-muted-foreground text-xs break-words">
                    {line}
                  </p>
                ))}
              </div>
            )
          )}
          {outcome?.summary ? (
            <div className="flex min-w-0 flex-col gap-0.5">
              <p data-slot="job-outcome" className="text-label break-words">
                {outcome.summary}
              </p>
              {detail && (
                <p data-slot="job-detail" className="text-muted-foreground text-xs break-words">
                  {detail}
                </p>
              )}
            </div>
          ) : null}
          {proposed && (detail || warning) && (
            <div className="flex min-w-0 flex-col gap-0.5 text-xs">
              {detail && <p data-slot="job-detail">{detail}</p>}
              {warning && (
                <p data-slot="job-warning" className="text-muted-foreground">
                  {warning}
                </p>
              )}
            </div>
          )}
          {actions ? <div className="pointer-events-auto flex flex-wrap items-center gap-2 self-start">{actions}</div> : null}
        </div>
      )}
      <span className="sr-only" role="status">
        {announcement}
      </span>
    </div>
  );
}
