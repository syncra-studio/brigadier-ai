import type { ComponentProps, FC } from "react";

import { cn } from "@/lib/utils";

/**
 * A diff stat, "+88 −7": mono 12, tabular, green and red with a zero dim. `dim` greys both (a
 * reverted change); `quiet` keeps them grey until the row (`group`) is hovered.
 */
export const Counts: FC<
  Omit<ComponentProps<"span">, "children"> & { insertions: number; deletions: number; dim?: boolean; quiet?: boolean }
> = ({ insertions, deletions, dim = false, quiet = false, className, ...props }) => {
  const tone = (count: number, colour: string, hovered: string) =>
    dim || count === 0 ? "text-muted-foreground" : quiet ? cn("text-muted-foreground", hovered) : colour;
  return (
    <span {...props} className={cn("shrink-0 font-mono text-xs tabular-nums", className)}>
      <span className={tone(insertions, "text-success", "group-hover:text-success")}>+{insertions}</span>{" "}
      <span className={tone(deletions, "text-destructive", "group-hover:text-destructive")}>−{deletions}</span>
    </span>
  );
};
