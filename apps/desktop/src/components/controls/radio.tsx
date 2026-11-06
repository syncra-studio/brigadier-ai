import type * as React from "react";
import { RadioGroup as RadioGroupPrimitive } from "radix-ui";

import { cn } from "@/lib/utils";

/** One choice of several; arrow keys move it. */
function RadioGroup(props: React.ComponentProps<typeof RadioGroupPrimitive.Root>) {
  return <RadioGroupPrimitive.Root data-slot="radio-group" {...props} />;
}

/** A small round mark that fills with the switch's blue around a white dot when chosen. */
function Radio({ className, ...props }: React.ComponentProps<typeof RadioGroupPrimitive.Item>) {
  return (
    <RadioGroupPrimitive.Item
      data-slot="radio"
      className={cn(
        "border-input data-[state=checked]:bg-toggle-on data-[state=checked]:border-toggle-on flex size-icon-md shrink-0 items-center justify-center rounded-full border",
        className,
      )}
      {...props}
    >
      <RadioGroupPrimitive.Indicator className="bg-foreground size-1.5 rounded-full" />
    </RadioGroupPrimitive.Item>
  );
}

export { Radio, RadioGroup };
