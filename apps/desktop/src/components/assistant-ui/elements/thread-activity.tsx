import { ChevronRight } from "@openai/apps-sdk-ui/components/Icon";
import type { ComponentProps, ReactNode } from "react";

import { ACTIVITY_DETAIL, ACTIVITY_ROW } from "@/components/assistant-ui/elements/activity-row";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { cn } from "@/lib/utils";

/** The same one-line activity disclosure in the parent and read-only worker threads. */
export function ThreadActivity({ detail, detailClassName, className, children, ...props }: ComponentProps<"div"> & {
  detail?: ReactNode;
  detailClassName?: string;
}) {
  const row = className ?? ACTIVITY_ROW;
  if (!detail) return <div className={row} {...props}>{children}</div>;
  return <Collapsible {...props}>
    <CollapsibleTrigger asChild>
      <div role="button" tabIndex={0}
        onKeyDown={(event) => {
          if (event.target === event.currentTarget && (event.key === "Enter" || event.key === " ")) {
            event.preventDefault();
            event.currentTarget.click();
          }
        }}
        className={cn(row, "group hover:text-foreground focus-visible:ring-ring/50 rounded-control w-full cursor-pointer text-start outline-none focus-visible:ring-1")}>
        {children}
        <ChevronRight aria-hidden className="size-icon-xs shrink-0 opacity-0 transition-[rotate,opacity] group-hover:opacity-100 group-focus-visible:opacity-100 group-data-[state=open]:rotate-90 group-data-[state=open]:opacity-100" />
      </div>
    </CollapsibleTrigger>
    <CollapsibleContent className={detailClassName ?? ACTIVITY_DETAIL}>{detail}</CollapsibleContent>
  </Collapsible>;
}
