import type * as React from "react";

import { cn } from "@/lib/utils";

function Kbd({ className, ...props }: React.ComponentProps<"kbd">) {
  return (
    <kbd
      data-slot="kbd"
      className={cn(
        "text-muted-foreground border-border/60 bg-muted/50 h-kbd min-w-kbd pointer-events-none inline-flex w-fit items-center justify-center rounded-xs border px-1 font-mono text-2xs select-none",
        "[&_svg:not([class*='size-'])]:size-icon-xs",
        // On a button, a tint of the button's own text colour, so it reads on light and dark
        // buttons alike, in the label's font.
        "in-data-[slot=button]:bg-current/12 in-data-[slot=button]:text-current in-data-[slot=button]:border-transparent in-data-[slot=button]:font-sans in-data-[slot=button]:text-xs in-data-[slot=button]:leading-none",
        // In a tooltip, a grey pill after the tip.
        "[[data-slot=tooltip-content]_&]:bg-foreground/10 [[data-slot=tooltip-content]_&]:text-popover-foreground [[data-slot=tooltip-content]_&]:border-transparent [[data-slot=tooltip-content]_&]:px-1.5 [[data-slot=tooltip-content]_&]:font-sans [[data-slot=tooltip-content]_&]:text-xs",
        className,
      )}
      {...props}
    />
  );
}

export { Kbd };
