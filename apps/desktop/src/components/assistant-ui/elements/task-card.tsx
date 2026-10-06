import { ChevronRight } from "@openai/apps-sdk-ui/components/Icon";
import type { ComponentProps, ReactNode } from "react";

import { cn } from "@/lib/utils";

/**
 * A worker's row in the thread's work (the Task element of assistant-ui, as an activity row on
 * Brigadier's tokens): one line naming the worker and what it is at ("[Add tests] failed"),
 * with its controls at the end. The line opens the details under it; a chevron shows while
 * the line is hovered or open. What needs the user (why it waits, why it failed) shows under
 * the line, open or not. The details render only while open.
 */
export function TaskCard({
  name,
  status,
  label,
  actions,
  result,
  open,
  onOpenChange,
  children,
  className,
  ...props
}: Omit<ComponentProps<"div">, "children" | "title"> & {
  /** Who: the worker's chip, which opens it. */
  name: ReactNode;
  /** What it is at, after its name ("is working", "failed"). */
  status: ReactNode;
  /** The row's name for the toggle ("Add tests"). */
  label: string;
  actions?: ReactNode;
  /** A line that needs the user: why it waits, why it failed. */
  result?: ReactNode;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  children?: ReactNode;
}) {
  return (
    <div data-slot="task-card" className={cn("flex w-full min-w-0 flex-col", className)} {...props}>
      <div className="min-h-row-sm flex min-w-0 items-center gap-2">
        <div className="group/task-header relative flex min-w-0 items-center text-sm">
          {/* The whole line toggles; its chip and links stay clickable above it. */}
          <button
            type="button"
            aria-expanded={open}
            aria-label={`${open ? "Hide" : "Show"} details of ${label}`}
            onClick={() => onOpenChange(!open)}
            className="rounded-control absolute inset-0"
          />
          <span className="pointer-events-none relative flex min-w-0 items-center gap-1.5 [&_a]:pointer-events-auto [&_button]:pointer-events-auto">
            {name}
            {status}
            <ChevronRight
              aria-hidden
              className={cn(
                "text-muted-foreground size-icon-xs shrink-0 transition-transform duration-300 motion-reduce:transition-none",
                open
                  ? "rotate-90"
                  : "opacity-0 group-hover/task-header:opacity-100 group-has-focus-visible/task-header:opacity-100",
              )}
            />
          </span>
        </div>
        {actions && (
          <div data-slot="task-card-actions" className="ms-auto flex shrink-0 items-center gap-0.5">
            {actions}
          </div>
        )}
      </div>
      {result && (
        <div data-slot="task-card-result" className="ps-6 text-sm wrap-break-word">
          {result}
        </div>
      )}
      {open && children && (
        <div
          data-slot="task-card-body"
          className="animate-fold-open motion-reduce:animate-fold-fade-in grid grid-rows-1"
        >
          <div className="flex min-h-0 flex-col gap-3 overflow-hidden ps-6 pt-1 pb-2">{children}</div>
        </div>
      )}
    </div>
  );
}
