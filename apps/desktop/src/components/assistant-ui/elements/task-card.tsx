// Ported from assistant-ui’s standalone registry (MIT); see THIRD_PARTY_NOTICES.md.
// The rows of the context card's Activities section: icons map to the app's, raw sizes and
// colours to its tokens. Shaped as a context-card row rather than a card: no surface or border, the
// row's height and wash, the meta as a second line under the label, and opened, one line of
// detail with its actions inline where the original stacks actions and a transcript.

import { Children, type ComponentProps, type ReactNode, useState } from "react";
import { Check as CheckIcon, ChevronRight as ChevronRightIcon, StopCircle as Ban, X as XIcon } from "@openai/apps-sdk-ui/components/Icon";
import { summaryRowInteractive } from "@/components/assistant-ui/elements/summary-section";
import { Spinner as Loader2Icon } from "@/components/glyphs/spinner";
import { cn } from "@/lib/utils";

export type TaskCardState =
  | "working"
  | "waiting"
  | "done"
  | "failed"
  | "cancelled";

const isRenderable = (node: ReactNode) =>
  node !== undefined && node !== null && node !== false && node !== true;

export function TaskStateIcon({
  state,
  className,
}: {
  state: TaskCardState;
  className?: string;
}) {
  if (state === "done") {
    return (
      <CheckIcon
        aria-hidden
        className={cn("size-icon-xs shrink-0 text-success", className)}
      />
    );
  }
  if (state === "failed") {
    return (
      <XIcon
        aria-hidden
        className={cn("text-destructive size-icon-xs shrink-0", className)}
      />
    );
  }
  if (state === "cancelled") {
    return (
      <Ban
        aria-hidden
        className={cn("text-muted-foreground size-icon-xs shrink-0", className)}
      />
    );
  }
  if (state === "working") {
    return (
      <Loader2Icon
        aria-hidden
        className={cn(
          "text-muted-foreground size-icon-xs shrink-0 animate-spin motion-reduce:animate-none",
          className,
        )}
      />
    );
  }
  return (
    <span
      aria-hidden
      className={cn(
        "border-foreground/35 m-1 size-1.5 shrink-0 rounded-capsule border",
        className,
      )}
    />
  );
}

export function TaskCard({
  label,
  meta,
  state,
  elapsed,
  actions,
  result,
  open,
  onOpenChange,
  children,
  className,
  ...props
}: Omit<
  ComponentProps<"div">,
  "children" | "label" | "state" | "result" | "open" | "onOpenChange"
> & {
  label: string;
  meta?: string | undefined;
  state: TaskCardState;
  elapsed?: string | undefined;
  actions?: ReactNode | undefined;
  result?: ReactNode | undefined;
  open?: boolean | undefined;
  onOpenChange?: ((open: boolean) => void) | undefined;
  children?: ReactNode | undefined;
}) {
  const hasTranscript = Children.toArray(children).length > 0;
  const inert = open !== undefined && onOpenChange === undefined;
  const [uncontrolledOpen, setUncontrolledOpen] = useState(false);
  const isOpen = open ?? uncontrolledOpen;
  const toggle = () => {
    const next = !isOpen;
    if (open === undefined) setUncontrolledOpen(next);
    onOpenChange?.(next);
  };

  return (
    <div
      data-slot="task-card"
      data-state={state}
      className={cn("flex w-full min-w-0 flex-col", className)}
      {...props}
    >
      <button
        type="button"
        aria-expanded={hasTranscript ? isOpen : undefined}
        disabled={!hasTranscript || inert}
        onClick={toggle}
        data-state={isOpen ? "open" : "closed"}
        className={cn(
          summaryRowInteractive,
          "relative isolate flex min-h-row-sm w-full min-w-0 items-center gap-2 py-1 text-start text-label disabled:cursor-default",
        )}
      >
        <span className="flex min-w-icon-md shrink-0 items-center justify-center">
          <TaskStateIcon state={state} />
        </span>
        <span className="sr-only">{state}</span>
        <span className="flex min-w-0 flex-1 flex-col gap-0.5">
          <span className="truncate">{label}</span>
          {meta !== undefined && (
            <span className="text-muted-foreground truncate text-xs">{meta}</span>
          )}
        </span>
        {elapsed !== undefined && (
          <span className="text-muted-foreground/70 shrink-0 text-xs tabular-nums">
            {elapsed}
          </span>
        )}
        {hasTranscript && (
          <ChevronRightIcon
            aria-hidden
            className={cn(
              "text-muted-foreground size-icon-xs shrink-0 transition-transform duration-200 motion-reduce:transition-none",
              isOpen && "rotate-90",
            )}
          />
        )}
      </button>
      {hasTranscript && isOpen && (
        <div
          data-slot="task-card-transcript"
          className="flex min-w-0 items-center gap-2 pb-1"
        >
          {/* Under the label, past the state icon's slot. */}
          <span aria-hidden className="min-w-icon-md shrink-0" />
          {children}
          {isRenderable(actions) && (
            <div data-slot="task-card-actions" className="flex shrink-0 items-center gap-0.5">
              {actions}
            </div>
          )}
        </div>
      )}
      {isRenderable(result) && (
        <div
          data-slot="task-card-result"
          className="text-muted-foreground flex min-w-0 gap-2 pb-1 text-xs"
        >
          <span aria-hidden className="min-w-icon-md shrink-0" />
          <span className="min-w-0 truncate">{result}</span>
        </div>
      )}
    </div>
  );
}
