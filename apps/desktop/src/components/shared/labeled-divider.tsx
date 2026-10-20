import type { ComponentProps } from "react";

import { cn } from "@/lib/utils";

/**
 * Words across the column between two hairlines ("Claude usage limit reset · 9:39 PM"): a
 * label, and an icon or a link as its caller puts them in.
 */
export function LabeledDivider({ className, children, ...props }: ComponentProps<"div">) {
  return (
    <div
      data-slot="labeled-divider"
      {...props}
      className={cn("text-muted-foreground flex items-center gap-3 self-stretch py-2 text-sm", className)}
    >
      <span aria-hidden data-rule className="bg-border h-px flex-1" />
      {children}
      <span aria-hidden data-rule className="bg-border h-px flex-1" />
    </div>
  );
}
