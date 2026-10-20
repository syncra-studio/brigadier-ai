import type * as React from "react";
import { ToggleGroup as ToggleGroupPrimitive } from "radix-ui";

import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

/** A row of choices, one picked at a time (no look of its own). */
function SegmentedRoot(props: React.ComponentProps<typeof ToggleGroupPrimitive.Root>) {
  return <ToggleGroupPrimitive.Root {...props} />;
}

/** One choice in a `SegmentedRoot` (no look of its own). */
function Segment(props: React.ComponentProps<typeof ToggleGroupPrimitive.Item>) {
  return <ToggleGroupPrimitive.Item {...props} />;
}

/** `tip` names a choice whose label is only an icon, and shows as its tooltip. */
type Choice<T extends string> = { value: T; label: React.ReactNode; hint?: React.ReactNode; tip?: string };

/**
 * One of a few choices as pills side by side; the chosen one is filled. With `fill` the pills
 * share the whole width, on a track.
 */
function Segmented<T extends string>({
  label,
  value,
  options,
  onChange,
  disabled,
  fill = false,
}: {
  label: string;
  value: T;
  options: readonly Choice<T>[];
  onChange: (value: T) => void;
  disabled?: boolean;
  fill?: boolean;
}) {
  return (
    <SegmentedRoot
      type="single"
      aria-label={label}
      value={value}
      disabled={disabled ?? false}
      // A single choice is always made: pressing the chosen pill again keeps it.
      onValueChange={(next) => next && onChange(next as T)}
      className={cn(
        "flex max-w-full min-w-0 items-center gap-0.5",
        fill && "bg-foreground/5 rounded-capsule w-full p-0.5",
      )}
    >
      {options.map((option) => {
        const segment = (
          <Segment
            key={option.value}
            value={option.value}
            aria-label={option.tip}
            className={cn(
              "text-foreground/50 hover:text-foreground data-[state=on]:bg-foreground/5 data-[state=on]:text-foreground rounded-capsule text-label h-6 shrink-0 border border-transparent px-2 whitespace-nowrap transition-colors disabled:opacity-50",
              fill && "data-[state=on]:bg-foreground/10 min-w-0 flex-auto truncate",
            )}
          >
            {option.label}
          </Segment>
        );
        return option.tip ? (
          <Tooltip key={option.value}>
            <TooltipTrigger asChild>{segment}</TooltipTrigger>
            <TooltipContent>{option.tip}</TooltipContent>
          </Tooltip>
        ) : (
          segment
        );
      })}
    </SegmentedRoot>
  );
}

export { Segment, Segmented, SegmentedRoot, type Choice };
