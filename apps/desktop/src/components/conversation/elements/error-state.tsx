import { type ComponentProps, useState } from "react";

import { CHEVRON, ROW, ROW_TOGGLE } from "@/components/conversation/elements/activity-row";
import { TooltipIconButton } from "@/components/controls/tooltip-icon-button";
import { Check, ChevronRight, Copy, Regenerate } from "@/components/icons";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { cn } from "@/lib/utils";

/**
 * What went wrong with a turn or a worker's step, said calmly: one quiet line of the thread, never
 * a card, with Try again where trying again exists and Details opening the error's own words (with
 * Copy) under it. Only its sentence is red: a stopped turn is part of the thread, not an alarm.
 * While it retries it becomes a shimmering "Retrying" line.
 */

export type ErrorStateProps = Omit<ComponentProps<"div">, "children" | "role" | "title"> & {
  /** What happened, in plain words. */
  summary: string;
  /** The error's own words, in full: shown only once Details opens. */
  details?: string | null | undefined;
  retrying?: boolean | undefined;
  /** Omitted where nothing can be tried again. */
  onRetry?: (() => void) | undefined;
  /** The retry button's word: "Try again" unless the turn goes on instead ("Continue"). */
  retryLabel?: string | undefined;
};

export function ErrorState({ summary, details, retrying = false, onRetry, retryLabel = "Try again", className, ...props }: ErrorStateProps) {
  const { isCopied, copyToClipboard } = useCopyToClipboard();
  const [open, setOpen] = useState(false);
  if (retrying) {
    return (
      <div
        data-slot="error-state"
        key="retrying"
        role="status"
        className={cn(
          "fade-in animate-in text-muted-foreground flex w-full items-center gap-2.5 text-sm duration-300 motion-reduce:animate-none",
          className,
        )}
        {...props}
      >
        <Regenerate aria-hidden className="size-icon-sm shrink-0 animate-spin motion-reduce:animate-none" />
        <span className="shimmer">Retrying</span>
      </div>
    );
  }
  const text = details?.trim() ?? "";
  return (
    <div
      data-slot="error-state"
      key="error"
      role="status"
      className={cn("fade-in animate-in flex w-full min-w-0 flex-col gap-1 duration-300 motion-reduce:animate-none", className)}
      {...props}
    >
      <div className={cn(ROW, "flex-wrap")}>
        <p data-slot="error-state-summary" className="text-destructive min-w-0">
          {summary}
        </p>
        {onRetry && (
          <button type="button" onClick={onRetry} className="text-link hover:text-link/80 shrink-0">
            {retryLabel}
          </button>
        )}
        {text && (
          <button
            type="button"
            data-slot="error-state-details"
            data-state={open ? "open" : "closed"}
            aria-expanded={open}
            onClick={() => setOpen(!open)}
            className={cn(ROW_TOGGLE, "flex shrink-0 items-center gap-1.5")}
          >
            Details
            <ChevronRight aria-hidden className={CHEVRON} />
          </button>
        )}
      </div>
      {text && open && (
        <div data-slot="error-state-raw" className="relative">
          <pre className="text-foreground/60 max-h-60 overflow-y-auto pe-9 font-mono text-xs leading-snug whitespace-pre-wrap wrap-anywhere">
            {text}
          </pre>
          <TooltipIconButton
            tooltip={isCopied ? "Copied" : "Copy"}
            size="icon-xs"
            className="text-muted-foreground absolute end-1 top-0"
            onClick={() => copyToClipboard(text)}
          >
            {isCopied ? <Check /> : <Copy />}
          </TooltipIconButton>
        </div>
      )}
    </div>
  );
}
