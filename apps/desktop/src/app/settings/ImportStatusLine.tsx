import type { ReactNode } from "react";

import { cn } from "@/lib/utils";

/** Import Browser Data's line in place of the list: why it failed (`error`), or none found. */
export function ImportStatusLine({ error = false, children }: { error?: boolean; children: ReactNode }) {
  return <p className={cn(error ? "text-destructive" : "text-foreground/65", "px-1 text-sm")}>{children}</p>;
}
