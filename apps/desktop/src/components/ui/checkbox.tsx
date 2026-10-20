import type * as React from "react";
import { Checkbox as CheckboxPrimitive } from "radix-ui";

import { Check } from "@/components/ui/icon";
import { cn } from "@/lib/utils";

/** The one checkbox: a small square that fills with the switch's blue and shows a white tick when checked. */
function Checkbox({ className, ...props }: React.ComponentProps<typeof CheckboxPrimitive.Root>) {
  return (
    <CheckboxPrimitive.Root
      data-slot="checkbox"
      className={cn(
        "border-input data-[state=checked]:bg-toggle-on data-[state=checked]:border-toggle-on data-[state=checked]:text-foreground flex size-icon-md shrink-0 items-center justify-center rounded-checkbox border disabled:cursor-not-allowed",
        className,
      )}
      {...props}
    >
      <CheckboxPrimitive.Indicator>
        <Check className="size-icon-xs" />
      </CheckboxPrimitive.Indicator>
    </CheckboxPrimitive.Root>
  );
}

export { Checkbox };
