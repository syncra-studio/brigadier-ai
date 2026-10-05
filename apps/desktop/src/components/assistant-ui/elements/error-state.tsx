import { ExclamationMarkCircle, Regenerate } from "@openai/apps-sdk-ui/components/Icon";
import { type ComponentProps, useState } from "react";

import { cn } from "@/lib/utils";

/**
 * What went wrong with a reply or a worker's step: a title, the full error (folded to two lines
 * when it is long, with Show more), and Retry where trying again exists. While it retries it
 * becomes a shimmering "Retrying" line.
 */

/** An error longer than this, or of more than one line, folds. */
const LONG = 160;

export type ErrorStateProps = Omit<ComponentProps<"div">, "children" | "role" | "title"> & {
  title: string;
  /** The error's own words, in full. */
  detail?: string | null | undefined;
  retrying?: boolean | undefined;
  /** Omitted where nothing can be tried again. */
  onRetry?: (() => void) | undefined;
};

export function ErrorState({ title, detail, retrying = false, onRetry, className, ...props }: ErrorStateProps) {
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
  const text = detail?.trim() ?? "";
  const long = text.length > LONG || text.includes("\n");
  return (
    <div
      data-slot="error-state"
      key="error"
      role="alert"
      className={cn(
        "fade-in animate-in bg-destructive/10 rounded-control flex w-full items-start gap-2.5 px-3 py-2.5 text-sm duration-300 motion-reduce:animate-none",
        className,
      )}
      {...props}
    >
      <ExclamationMarkCircle aria-hidden className="text-destructive mt-0.5 size-icon-sm shrink-0" />
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <p className="text-destructive font-medium">{title}</p>
        {text && (
          <p
            className={cn(
              "text-destructive/75 leading-snug whitespace-pre-wrap wrap-anywhere",
              long && !open && "line-clamp-2",
            )}
          >
            {text}
          </p>
        )}
        {long && (
          <button
            type="button"
            aria-expanded={open}
            onClick={() => setOpen(!open)}
            className="text-destructive/75 hover:text-destructive focus-visible:ring-ring/50 rounded-control w-fit text-xs outline-none focus-visible:ring-1"
          >
            {open ? "Show less" : "Show more"}
          </button>
        )}
      </div>
      {onRetry && (
        <button
          type="button"
          onClick={onRetry}
          className="text-destructive hover:bg-destructive/10 focus-visible:ring-ring/50 rounded-capsule ms-auto flex shrink-0 items-center gap-1.5 px-3 py-1 text-xs font-medium outline-none transition-colors focus-visible:ring-1"
        >
          <Regenerate aria-hidden className="size-icon-xs" />
          Retry
        </button>
      )}
    </div>
  );
}
