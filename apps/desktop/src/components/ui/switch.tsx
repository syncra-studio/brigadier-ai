import type * as React from "react";
import { Switch as SwitchPrimitive } from "radix-ui";

import { cn } from "@/lib/utils";

/** An on/off switch: blue when on, its thumb sliding across in 150ms. */
function Switch({ className, ...props }: React.ComponentProps<typeof SwitchPrimitive.Root>) {
  return (
    <SwitchPrimitive.Root
      data-slot="switch"
      className={cn(
        "peer bg-foreground/10 data-[state=checked]:bg-toggle-on rounded-capsule inline-flex h-switch-height w-switch-width shrink-0 items-center transition-colors duration-150 ease-out disabled:cursor-not-allowed disabled:opacity-60",
        className,
      )}
      {...props}
    >
      <SwitchPrimitive.Thumb
        data-slot="switch-thumb"
        className="rounded-capsule block size-switch-thumb translate-x-0.5 border-foreground bg-foreground border shadow-thumb transition-transform duration-150 ease-out data-[state=checked]:translate-x-switch-travel rtl:-translate-x-0.5 rtl:data-[state=checked]:-translate-x-switch-travel"
      />
    </SwitchPrimitive.Root>
  );
}

export { Switch };
