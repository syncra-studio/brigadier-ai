import { ChevronRightAlt } from "@openai/apps-sdk-ui/components/Icon";
import { type ComponentProps, createContext, type ReactNode, useContext, useRef } from "react";

import { CHEVRON, OPENS, ROW, ROW_DETAIL, ROW_TOGGLE } from "@/components/assistant-ui/elements/activity-row";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { cn } from "@/lib/utils";

/** The list an opened group's rows scroll in. */
export const GROUP_LIST = "work-group-list";

/** Told when a row in it opens or closes: a lone step's group keeps an opened row as it is. */
export const RowOpens = createContext<((open: boolean) => void) | null>(null);

/**
 * Scrolls `row` into its group's list, nearest, so what it opened to shows whole (its state
 * line too) when it fits; only that list moves, never the thread.
 */
function showInGroup(row: HTMLElement) {
  const list = row.closest<HTMLElement>(`[data-slot=${GROUP_LIST}]`);
  if (!list) return;
  const shown = list.getBoundingClientRect();
  const { top, bottom } = row.getBoundingClientRect();
  if (bottom > shown.bottom) list.scrollTop += Math.min(bottom - shown.bottom, top - shown.top);
  else if (top < shown.top) list.scrollTop -= shown.top - top;
}

/** One row of a thread's activity that opens to its detail when it has one. */
export function ThreadActivity({ detail, detailClassName, defaultOpen = false, className, children, onOpenChange, ...props }: ComponentProps<"div"> & {
  detail?: ReactNode;
  detailClassName?: string;
  /** Opened from the start, as if the user had opened it. */
  defaultOpen?: boolean;
  /** The row opened or closed. */
  onOpenChange?: (open: boolean) => void;
}) {
  const root = useRef<HTMLDivElement>(null);
  const told = useContext(RowOpens);
  const row = className ?? ROW;
  if (!detail) return <div className={row} {...props}>{children}</div>;
  // Opened inside a group, it shows itself in the group's list: at once, and again once its detail has its height.
  const opened = (open: boolean) => {
    onOpenChange?.(open);
    told?.(open);
    if (open) requestAnimationFrame(() => root.current && showInGroup(root.current));
  };
  return <Collapsible ref={root} {...props} defaultOpen={defaultOpen} onOpenChange={opened}>
    <CollapsibleTrigger asChild>
      <div role="button" tabIndex={0}
        onKeyDown={(event) => {
          if (event.target === event.currentTarget && (event.key === "Enter" || event.key === " ")) {
            event.preventDefault();
            event.currentTarget.click();
          }
        }}
        className={cn(row, ROW_TOGGLE)}>
        {children}
        <ChevronRightAlt aria-hidden className={CHEVRON} />
      </div>
    </CollapsibleTrigger>
    <CollapsibleContent className={OPENS} onAnimationEnd={(event) => {
      if (event.target === event.currentTarget && root.current?.dataset["state"] === "open") showInGroup(root.current);
    }}>
      <div className={detailClassName ?? ROW_DETAIL}>{detail}</div>
    </CollapsibleContent>
  </Collapsible>;
}
