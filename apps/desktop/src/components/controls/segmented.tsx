import type * as React from "react";
import { useId } from "react";
import { ToggleGroup as ToggleGroupPrimitive } from "radix-ui";

import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/overlays/tooltip";
import { cn } from "@/lib/utils";

/** A row of choices, one picked at a time (no look of its own). */
function SegmentedRoot(props: React.ComponentProps<typeof ToggleGroupPrimitive.Root>) {
  return <ToggleGroupPrimitive.Root {...props} />;
}

/** One choice in a `SegmentedRoot` (no look of its own). */
function Segment(props: React.ComponentProps<typeof ToggleGroupPrimitive.Item>) {
  return <ToggleGroupPrimitive.Item {...props} />;
}

/**
 * `tip` names a choice whose label is only an icon, and shows as its tooltip. `reason` says why
 * a choice can't be picked: it shows greyed, is skipped by a press and keeps its place in the
 * arrow keys' order, with the reason as its tooltip and its description.
 */
type Choice<T extends string> = {
  value: T;
  label: React.ReactNode;
  hint?: React.ReactNode;
  tip?: string;
  reason?: string | null;
};

/**
 * One of a few choices as pills side by side; the chosen one is filled. With `fill` the pills
 * share the whole width, on a track. Exactly one is chosen at a time: pressing the chosen pill
 * again keeps it, and a choice with a `reason` is never chosen by a press.
 */
function Segmented<T extends string>({
  label,
  value,
  options,
  onChange,
  disabled,
  fill = false,
  className,
}: {
  label: string;
  value: T;
  options: readonly Choice<T>[];
  onChange: (value: T) => void;
  disabled?: boolean;
  fill?: boolean;
  className?: string;
}) {
  const id = useId();
  return (
    <SegmentedRoot
      type="single"
      aria-label={label}
      value={value}
      disabled={disabled ?? false}
      onValueChange={(next) => {
        if (!next || options.find((option) => option.value === next)?.reason) return;
        onChange(next as T);
      }}
      className={cn(
        "flex max-w-full min-w-0 items-center gap-0.5",
        fill && "bg-foreground/5 rounded-capsule w-full p-0.5",
        className,
      )}
    >
      {options.map((option) => {
        const reason = option.reason ?? null;
        const segment = (
          <Segment
            key={option.value}
            value={option.value}
            aria-label={option.tip}
            aria-disabled={reason ? true : undefined}
            aria-describedby={reason ? `${id}-${option.value}` : undefined}
            className={cn(
              "text-foreground/50 hover:text-foreground data-[state=on]:bg-foreground/5 data-[state=on]:text-foreground rounded-capsule text-label h-6 shrink-0 border border-transparent px-2 whitespace-nowrap transition-colors disabled:opacity-50",
              "aria-disabled:hover:text-foreground/50 aria-disabled:cursor-default aria-disabled:opacity-50",
              fill && "data-[state=on]:bg-foreground/10 min-w-0 flex-auto truncate",
            )}
          >
            {option.label}
            {reason && (
              <span id={`${id}-${option.value}`} className="sr-only">
                {reason}
              </span>
            )}
          </Segment>
        );
        const tip = reason ?? option.tip;
        return tip ? (
          <Tooltip key={option.value}>
            <TooltipTrigger asChild>{segment}</TooltipTrigger>
            <TooltipContent>{tip}</TooltipContent>
          </Tooltip>
        ) : (
          segment
        );
      })}
    </SegmentedRoot>
  );
}

export { Segment, Segmented, SegmentedRoot, type Choice };
