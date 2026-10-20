import type { ComponentProps } from "react";

import { TooltipIconButton } from "@/components/assistant-ui/tooltip-icon-button";
import { Button } from "@/components/ui/button";
import { Check, ChevronRight, Copy, Regenerate, Warning } from "@/components/ui/icon";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { cn } from "@/lib/utils";

/**
 * What went wrong with a turn or a worker's step, said calmly: one plain line, the error's own
 * words folded under Details (with Copy), and Try again where trying again exists. Never red: a
 * stopped turn is part of the thread, not an alarm. While it retries it becomes a shimmering
 * "Retrying" line.
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
      className={cn(
        "fade-in animate-in bg-muted text-foreground rounded-control flex w-full flex-col gap-1 px-3 py-2 text-sm duration-300 motion-reduce:animate-none",
        className,
      )}
      {...props}
    >
      <div className="flex min-w-0 items-center gap-2.5">
        <Warning aria-hidden className="text-muted-foreground size-icon-sm shrink-0" />
        <p data-slot="error-state-summary" className="min-w-0 flex-1 leading-snug">
          {summary}
        </p>
        {onRetry && (
          <Button variant="outline" size="xs" onClick={onRetry} className="ms-auto">
            <Regenerate aria-hidden />
            {retryLabel}
          </Button>
        )}
      </div>
      {text && (
        <details data-slot="error-state-details" className="group/details ps-6.5">
          {/* Folds like a report's Details: its word and a quiet chevron, never the browser's marker. */}
          <summary className="text-muted-foreground hover:text-foreground rounded-control flex w-fit cursor-pointer list-none items-center gap-0.5 text-xs [&::-webkit-details-marker]:hidden">
            Details
            <ChevronRight
              aria-hidden
              className="size-chevron shrink-0 transition-[rotate] duration-150 ease-standard group-open/details:rotate-90 motion-reduce:transition-none"
            />
          </summary>
          <div className="bg-background/60 rounded-control relative mt-1.5">
            <pre className="text-muted-foreground max-h-60 overflow-y-auto py-2 ps-2.5 pe-9 font-mono text-xs leading-snug whitespace-pre-wrap wrap-anywhere">
              {text}
            </pre>
            <TooltipIconButton
              tooltip={isCopied ? "Copied" : "Copy"}
              size="icon-xs"
              className="text-muted-foreground absolute end-1 top-1"
              onClick={() => copyToClipboard(text)}
            >
              {isCopied ? <Check /> : <Copy />}
            </TooltipIconButton>
          </div>
        </details>
      )}
    </div>
  );
}
