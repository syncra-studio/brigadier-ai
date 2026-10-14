// Ported from assistant-ui’s standalone registry (MIT); see THIRD_PARTY_NOTICES.md.
// Icons and unavailable theme classes map to Brigadier equivalents. The extracted row adds
// a waiting state, accessible status and a children slot; the inbox adds children/actions slots.

import type { ComponentProps, ReactNode } from "react";
import { Check as CheckIcon, Pause as PauseIcon, X as XIcon } from "@openai/apps-sdk-ui/components/Icon";
import { Spinner as Loader2Icon } from "@/components/glyphs/spinner";
import { cn } from "@/lib/utils";
import { mono, paper } from "./surfaces";

export type BackgroundState = "running" | "waiting" | "ready" | "failed";

export interface BackgroundRun {
  id: string;
  title: string;
  state: BackgroundState;
  elapsed: string;
  summary?: string;
}

export function BackgroundInbox({
  runs,
  onCollect,
  className,
  children,
  actions,
  title = "Running elsewhere",
  summary,
  ...props
}: Omit<ComponentProps<"div">, "children" | "runs" | "onCollect" | "title"> & {
  runs: readonly BackgroundRun[];
  title?: ReactNode;
  summary?: ReactNode;
  children?: ReactNode;
  actions?: ReactNode;
  onCollect?: (id: string) => void;
}) {
  const ready = runs.filter((run) => run.state === "ready").length;
  const running = runs.filter((run) => run.state === "running").length;

  return (
    <div
      data-slot="background-inbox"
      className={cn(
        paper,
        "flex w-full max-w-96 flex-col gap-1 rounded-2xl p-3",
        className,
      )}
      {...props}
    >
      <div className="flex items-baseline justify-between px-1 pb-1">
        <span className="text-label font-medium">{title}</span>
        <span
          className={cn(
            mono,
            "tabular-nums",
            summary === undefined && ready > 0
              ? "text-link"
              : "text-muted-foreground",
          )}
        >
          {summary ?? (ready > 0 ? `${ready} ready` : `${running} in flight`)}
        </span>
      </div>

      {children ?? runs.map((run) => <BackgroundInboxRow key={run.id} run={run} onCollect={onCollect} />)}
      {actions && <div className="flex flex-wrap items-center justify-end gap-1 px-1 pt-1">{actions}</div>}
    </div>
  );
}

/** The registry row, with a details slot for preview controls, links and logs. */
export function BackgroundInboxRow({ run, onCollect, status, detail, children }: {
  run: BackgroundRun;
  onCollect?: ((id: string) => void) | undefined;
  status?: string;
  detail?: string;
  children?: ReactNode;
}) {
  const rowClassName = cn(
    "flex items-center gap-2.5 rounded-xl px-1.5 py-2 text-start transition-colors",
    run.state === "running"
      ? "cursor-default"
      : onCollect
        ? "hover:bg-foreground/5"
        : undefined,
  );
  const content = (
    <>
      <span data-slot="background-inbox-indicator" data-state={run.state} className="flex size-icon-sm shrink-0 items-center justify-center">
        {run.state === "running" ? (
          <Loader2Icon aria-hidden className="text-muted-foreground size-icon-xs animate-spin motion-reduce:animate-none" />
        ) : run.state === "waiting" ? (
          <PauseIcon aria-hidden className="text-muted-foreground size-icon-xs" />
        ) : run.state === "failed" ? (
          <XIcon aria-hidden className="size-icon-xs text-destructive" />
        ) : (
          <CheckIcon aria-hidden className="size-icon-xs text-success" />
        )}
      </span>

      <span className="sr-only">{status ?? run.state}. </span>
      <span className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span
          className={cn(
            "truncate text-label",
            run.state === "running"
              ? "text-muted-foreground"
              : "text-foreground/90",
          )}
        >
          {run.title}
        </span>
        {run.summary && (
          <span className={cn(mono, "text-muted-foreground truncate")}>
            {run.summary}
          </span>
        )}
      </span>

      <span
        className={cn(
          mono,
          "text-muted-foreground shrink-0 tabular-nums",
        )}
      >
        {run.elapsed}
      </span>
    </>
  );

  const row = onCollect ? (
    <button type="button" disabled={run.state === "running" || run.state === "waiting"}
      onClick={() => onCollect(run.id)} className={rowClassName} title={detail}>
      {content}
    </button>
  ) : (
    <div className={rowClassName} title={detail}>{content}</div>
  );
  return children ? (
    <article aria-label={run.title}>
      {row}
      <div className="px-1.5 pb-2 ps-7.5">{children}</div>
    </article>
  ) : row;
}
