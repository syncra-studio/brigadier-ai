import type { ReactNode } from "react";

import { cn } from "@/lib/utils";

/** The round badge at a summary's start. */
export function RoundBadge({ tone = "plain", children }: { tone?: "plain" | "done"; children: ReactNode }) {
  return (
    <span
      aria-hidden
      className={cn(
        "flex size-9 shrink-0 items-center justify-center rounded-full [&_svg]:size-icon-md",
        tone === "done" ? "bg-success/15 text-success" : "bg-foreground/8 text-foreground",
      )}
    >
      {children}
    </span>
  );
}
