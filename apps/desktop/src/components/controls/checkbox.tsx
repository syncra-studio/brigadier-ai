import type * as React from "react";
import { Checkbox as CheckboxPrimitive } from "radix-ui";

import { Check, Minus } from "@/components/icons";
import { cn } from "@/lib/utils";

const BOX = "border-input flex size-icon-md shrink-0 items-center justify-center rounded-checkbox border";
const FILLED = "bg-toggle-on border-toggle-on text-foreground";

/**
 * The one checkbox: a small square that fills with the switch's blue and shows a white tick when
 * checked, a white dash when mixed (`checked="indeterminate"`).
 */
function Checkbox({ className, ...props }: React.ComponentProps<typeof CheckboxPrimitive.Root>) {
  return (
    <CheckboxPrimitive.Root
      data-slot="checkbox"
      className={cn(
        BOX,
        "data-[state=checked]:bg-toggle-on data-[state=checked]:border-toggle-on data-[state=checked]:text-foreground",
        "data-[state=indeterminate]:bg-toggle-on data-[state=indeterminate]:border-toggle-on data-[state=indeterminate]:text-foreground",
        "disabled:cursor-not-allowed",
        className,
      )}
      {...props}
    >
      <CheckboxPrimitive.Indicator>
        {props.checked === "indeterminate" ? <Minus className="size-icon-xs" /> : <Check className="size-icon-xs" />}
      </CheckboxPrimitive.Indicator>
    </CheckboxPrimitive.Root>
  );
}

/** The checkbox's look alone, for a control that is a whole card (the card is the checkbox). */
function CheckboxMark({ checked, className }: { checked: boolean; className?: string }) {
  return (
    <span aria-hidden data-slot="checkbox-mark" className={cn(BOX, checked && FILLED, className)}>
      {checked && <Check className="size-icon-xs" />}
    </span>
  );
}

export { Checkbox, CheckboxMark };
