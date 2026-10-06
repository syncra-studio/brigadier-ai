import type * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { Slot } from "radix-ui";

import { cn } from "@/lib/utils";

const buttonVariants = cva(
  "aria-invalid:border-destructive aria-invalid:ring-destructive/40 rounded-control inline-flex shrink-0 items-center justify-center gap-1.5 text-sm font-medium whitespace-nowrap transition-all disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-icon-md",
  {
    variants: {
      variant: {
        default: "bg-primary text-primary-foreground hover:bg-primary/90",
        destructive:
          "bg-destructive/60 text-destructive-foreground hover:bg-destructive/70",
        outline:
          "bg-muted/50 text-foreground hover:bg-muted border-transparent",
        secondary:
          "bg-secondary text-secondary-foreground hover:bg-secondary/80",
        ghost: "hover:bg-accent/50 hover:text-accent-foreground",
        link: "text-primary underline-offset-4 hover:underline",
      },
      size: {
        default: "h-control-md px-button-md",
        xs: "h-control-xs px-button-xs gap-1 text-xs [&_svg:not([class*='size-'])]:size-icon-xs",
        sm: "h-control-sm px-button-sm gap-1",
        lg: "h-control-lg px-button-lg",
        icon: "size-icon-button-lg",
        "icon-xs":
          "size-icon-button-xs [&_svg:not([class*='size-'])]:size-icon-xs",
        "icon-sm":
          "size-icon-button-sm [&_svg:not([class*='size-'])]:size-icon-sm",
        "icon-md": "size-icon-button-md",
        "icon-lg": "size-icon-button-lg [&_svg:not([class*='size-'])]:size-icon-lg",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  },
);

function Button({
  className,
  variant = "default",
  size = "default",
  asChild = false,
  ...props
}: React.ComponentProps<"button"> &
  VariantProps<typeof buttonVariants> & {
    asChild?: boolean;
  }) {
  const Comp = asChild ? Slot.Root : "button";

  return (
    <Comp
      data-slot="button"
      data-variant={variant}
      data-size={size}
      className={cn(buttonVariants({ variant, size, className }))}
      {...props}
    />
  );
}

export { Button, buttonVariants };
