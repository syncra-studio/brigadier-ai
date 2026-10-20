import type * as React from "react";

import { cn } from "@/lib/utils";

/**
 * A key hint. `key` (the default) is a small grey key in the mono face; `chip` a capsule tinted
 * with the text's own colour, in the label's font (a card button's "Esc", a shortcut's keys).
 */
function Kbd({ className, variant = "key", ...props }: React.ComponentProps<"kbd"> & { variant?: "key" | "chip" }) {
  return (
    <kbd
      data-slot="kbd"
      data-variant={variant}
      className={cn(
        "h-kbd min-w-kbd pointer-events-none inline-flex w-fit items-center justify-center border select-none",
        "[&_svg:not([class*='size-'])]:size-icon-xs",
        variant === "chip"
          ? "rounded-capsule border-transparent bg-current/12 px-1.5 font-sans text-xs leading-none text-current"
          : [
              "text-muted-foreground border-border/60 bg-muted/50 rounded-xs px-1 font-mono text-2xs",
              // On a button, a chip: a tint of the button's own text colour, so it reads on light
              // and dark buttons alike, in the label's font.
              "in-data-[slot=button]:bg-current/12 in-data-[slot=button]:text-current in-data-[slot=button]:border-transparent in-data-[slot=button]:rounded-capsule in-data-[slot=button]:font-sans in-data-[slot=button]:text-xs in-data-[slot=button]:leading-none",
              // In a tooltip, a grey pill after the tip.
              "[[data-slot=tooltip-content]_&]:bg-foreground/10 [[data-slot=tooltip-content]_&]:text-popover-foreground [[data-slot=tooltip-content]_&]:border-transparent [[data-slot=tooltip-content]_&]:rounded-capsule [[data-slot=tooltip-content]_&]:px-1.5 [[data-slot=tooltip-content]_&]:font-sans [[data-slot=tooltip-content]_&]:text-xs",
            ],
        className,
      )}
      {...props}
    />
  );
}

export { Kbd };
