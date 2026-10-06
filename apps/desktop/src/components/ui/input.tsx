import type * as React from "react";

import { cn } from "@/lib/utils";

function Input({ className, type, ...props }: React.ComponentProps<"input">) {
  return (
    <input
      type={type}
      data-slot="input"
      className={cn(
        "placeholder:text-muted-foreground disabled:bg-input/80 aria-invalid:border-destructive/50 aria-invalid:ring-destructive/40 bg-muted/60 focus-visible:bg-background h-control-md rounded-control w-full min-w-0 border border-transparent px-2.5 py-1 text-sm transition-colors disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50 aria-invalid:ring-1",
        className,
      )}
      {...props}
    />
  );
}

export { Input };
