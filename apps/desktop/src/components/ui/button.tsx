import type * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { Slot } from "radix-ui";

import { cn } from "@/lib/utils";

const buttonVariants = cva(
  "aria-invalid:border-destructive aria-invalid:ring-destructive/40 inline-flex shrink-0 items-center justify-center gap-1.5 text-sm font-medium whitespace-nowrap transition-all disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-icon-md",
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
        // A row's action in Settings and the cards drawn like it: a faint wash, the label's
        // regular type.
        soft: "bg-foreground/5 text-foreground hover:bg-foreground/10 text-label font-normal",
        // The same, for an action that deletes or removes.
        "soft-destructive":
          "bg-destructive/10 text-destructive hover:bg-destructive/20 text-label font-normal",
        // Blue words that act (Show all, Add model); its icon, if any, is a child.
        "link-blue": "text-link hover:underline",
      },
      // Buttons with words are capsules; icon buttons have the icon-button corner (10px).
      size: {
        default: "rounded-capsule h-control-md px-button-md",
        xs: "rounded-capsule h-control-xs px-button-xs gap-1 text-xs [&_svg:not([class*='size-'])]:size-icon-xs",
        sm: "rounded-capsule h-control-sm px-button-sm gap-1",
        lg: "rounded-capsule h-control-lg px-button-lg",
        // Words with no box of their own (a row's "Show all"); `inline-flex` for words in a sentence.
        inline:
          "flex h-auto gap-1 p-0 text-xs font-medium [&_svg:not([class*='size-'])]:size-icon-xs",
        icon: "rounded-toolbar-button size-icon-button-lg",
        "icon-xs":
          "rounded-toolbar-button size-icon-button-xs [&_svg:not([class*='size-'])]:size-icon-xs",
        "icon-sm":
          "rounded-toolbar-button size-icon-button-sm [&_svg:not([class*='size-'])]:size-icon-sm",
        "icon-md": "rounded-toolbar-button size-icon-button-md",
        "icon-lg": "rounded-toolbar-button size-icon-button-lg [&_svg:not([class*='size-'])]:size-icon-lg",
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
  type,
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
      // A button submits its form only when it says so: one left untyped inside a form would be
      // the form's default button, which Enter in any of its fields clicks.
      type={asChild ? type : (type ?? "button")}
      {...props}
    />
  );
}

export { Button, buttonVariants };
