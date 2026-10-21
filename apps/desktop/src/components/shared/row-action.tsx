import type { ComponentProps, ReactNode } from "react";

import { cn } from "@/lib/utils";

const rowAction =
  "text-muted-foreground hover:text-foreground flex size-icon-button-xs shrink-0 items-center justify-center rounded-xs [&_svg]:size-icon-md";

/** An icon button on a row's end, shown while the row is hovered or focused. */
export function RowAction({ className, ...props }: ComponentProps<"button">) {
  return <button type="button" className={cn(rowAction, className)} {...props} />;
}

/** A row's hover actions, over its end; the row's status gives way to them. */
export function RowActionsSlot({ children }: { children: ReactNode }) {
  return (
    <div className="pointer-events-none absolute inset-y-0 end-1.5 flex items-center gap-2 opacity-0 group-focus-within/row:pointer-events-auto group-focus-within/row:opacity-100 group-hover/row:pointer-events-auto group-hover/row:opacity-100 has-data-[state=open]:pointer-events-auto has-data-[state=open]:opacity-100">
      {children}
    </div>
  );
}
