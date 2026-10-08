import { ChevronRight } from "@openai/apps-sdk-ui/components/Icon";
import type { ComponentProps, ReactNode } from "react";

import { CHEVRON, OPENS, ROW, ROW_DETAIL } from "@/components/assistant-ui/elements/activity-row";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { cn } from "@/lib/utils";

/** One row of a thread's activity that opens to its detail when it has one. */
export function ThreadActivity({ detail, detailClassName, className, children, ...props }: ComponentProps<"div"> & {
  detail?: ReactNode;
  detailClassName?: string;
}) {
  const row = className ?? ROW;
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
        className={cn(row, "group hover:text-foreground rounded-control w-full cursor-pointer text-start")}>
        {children}
        <ChevronRight aria-hidden className={CHEVRON} />
      </div>
    </CollapsibleTrigger>
    <CollapsibleContent className={OPENS}>
      <div className={detailClassName ?? ROW_DETAIL}>{detail}</div>
    </CollapsibleContent>
  </Collapsible>;
}
