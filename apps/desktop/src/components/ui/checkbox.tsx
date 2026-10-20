import type * as React from "react";
import { Checkbox as CheckboxPrimitive } from "radix-ui";

import { Check } from "@/components/ui/icon";
import { cn } from "@/lib/utils";

/** A checkbox: a small square that fills and shows a tick when checked. */
function Checkbox({ className, ...props }: React.ComponentProps<typeof CheckboxPrimitive.Root>) {
  return (
    <CheckboxPrimitive.Root
      data-slot="checkbox"
      className={cn(
        "border-input data-[state=checked]:bg-primary data-[state=checked]:border-primary data-[state=checked]:text-primary-foreground flex size-icon-md shrink-0 items-center justify-center rounded-xs border disabled:cursor-not-allowed",
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
