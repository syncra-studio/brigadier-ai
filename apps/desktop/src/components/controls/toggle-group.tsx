import * as React from "react";
import type { VariantProps } from "class-variance-authority";
import { ToggleGroup as ToggleGroupPrimitive } from "radix-ui";

import { cn } from "@/lib/utils";
import { toggleVariants } from "@/components/controls/toggle";

type ToggleGroupSpacing = "none" | "tight" | "loose";

const spacingClasses: Record<ToggleGroupSpacing, string> = {
  none: "gap-0",
  tight: "gap-0.5",
  loose: "gap-1",
};

const ToggleGroupContext = React.createContext<
  VariantProps<typeof toggleVariants> & {
    spacing: ToggleGroupSpacing;
  }
>({
  size: "default",
  variant: "default",
  spacing: "none",
});

function ToggleGroup({
  className,
  variant,
  size,
  spacing = "none",
  children,
  ...props
}: React.ComponentProps<typeof ToggleGroupPrimitive.Root> &
  VariantProps<typeof toggleVariants> & {
    spacing?: ToggleGroupSpacing;
  }) {
  const context = React.useMemo(
    () => ({ variant, size, spacing }),
    [variant, size, spacing],
  );

  return (
    <ToggleGroupPrimitive.Root
      data-slot="toggle-group"
      data-variant={variant}
      data-size={size}
      data-spacing={spacing}
      className={cn(
        "group/toggle-group rounded-control flex w-fit items-center",
        spacingClasses[spacing],
        className,
      )}
      {...props}
    >
      <ToggleGroupContext.Provider value={context}>
        {children}
      </ToggleGroupContext.Provider>
    </ToggleGroupPrimitive.Root>
  );
}

function ToggleGroupItem({
  className,
  children,
  variant,
  size,
  ...props
}: React.ComponentProps<typeof ToggleGroupPrimitive.Item> &
  VariantProps<typeof toggleVariants>) {
  const context = React.useContext(ToggleGroupContext);

  return (
    <ToggleGroupPrimitive.Item
      data-slot="toggle-group-item"
      data-variant={context.variant || variant}
      data-size={context.size || size}
      data-spacing={context.spacing}
      className={cn(
        toggleVariants({
          variant: context.variant || variant,
          size: context.size || size,
        }),
        "w-auto min-w-0 shrink-0 focus:z-10 focus-visible:z-10",
        "data-[spacing=none]:rounded-none data-[spacing=none]:first:rounded-s-control data-[spacing=none]:last:rounded-e-control data-[spacing=none]:data-[variant=outline]:border-s-0 data-[spacing=none]:data-[variant=outline]:first:border-s",
        className,
      )}
      {...props}
    >
      {children}
    </ToggleGroupPrimitive.Item>
  );
}

export { ToggleGroup, ToggleGroupItem };
