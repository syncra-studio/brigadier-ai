import type { ComponentProps } from "react";

import { cn } from "@/lib/utils";

/** One stacked share of a bar, in percent of its track. */
export type UsageBarPart = { key: string; value: number; className: string };

/**
 * A thin meter: a muted capsule track filled to `value` percent in `fill`. `parts` stack shares
 * end to end instead (square ends; a part too small to see still shows), and `indeterminate`
 * pulses a third while nothing is counted yet. Hidden from assistive tech unless given a `role`
 * (meter, progressbar) and its aria values.
 */
export function UsageBar({
  value = null,
  fill = "bg-primary",
  size = "sm",
  parts,
  indeterminate = false,
  className,
  ...props
}: Omit<ComponentProps<"span">, "children"> & {
  /** 0–100; `null` draws the empty track. */
  value?: number | null;
  fill?: string;
  /** sm 4px, md 6px. */
  size?: "sm" | "md";
  parts?: readonly UsageBarPart[];
  indeterminate?: boolean;
}) {
  return (
    <span
      data-slot="usage-bar"
      aria-hidden={props.role ? undefined : true}
      className={cn(
        "bg-muted rounded-capsule flex shrink-0 overflow-hidden",
        size === "sm" ? "h-1" : "h-1.5",
        className,
      )}
      {...props}
    >
      {parts ? (
        parts.map((part) => (
          <span
            key={part.key}
            data-part={part.key}
            className={cn("h-full min-w-0.5 shrink-0", part.className)}
            style={{ width: `${part.value}%` }}
          />
        ))
      ) : indeterminate ? (
        <span className={cn("rounded-capsule h-full w-1/3 animate-pulse", fill)} />
      ) : value === null ? null : (
        <span
          className={cn("rounded-capsule h-full transition-[width] motion-reduce:transition-none", fill)}
          style={{ width: `${value}%` }}
        />
      )}
    </span>
  );
}
