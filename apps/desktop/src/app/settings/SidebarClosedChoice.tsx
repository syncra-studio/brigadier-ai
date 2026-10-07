import { ToggleGroup as ToggleGroupPrimitive } from "radix-ui";

import { cn } from "@/lib/utils";
import type { SidebarCollapseMode } from "@/state/sidebar";

const CHOICES: { mode: SidebarCollapseMode; label: string }[] = [
  { mode: "strip", label: "Show icon strip" },
  { mode: "hidden", label: "Hide it completely" },
];

/** A tiny window: the strip down its left side, or nothing, beside a few lines of chat. */
function Picture({ strip }: { strip: boolean }) {
  return (
    <div className="border-divider bg-background flex h-14 w-24 gap-1 rounded-md border p-1">
      {strip && (
        <div className="bg-foreground/15 flex w-2.5 shrink-0 flex-col items-center gap-1 rounded-sm pt-1">
          <div className="bg-foreground/40 size-1 rounded-full" />
          <div className="bg-foreground/40 size-1 rounded-full" />
          <div className="bg-foreground/40 size-1 rounded-full" />
        </div>
      )}
      <div className="bg-foreground/5 flex flex-1 flex-col justify-end gap-1 rounded-sm p-1.5">
        <div className="bg-foreground/20 h-1 w-3/4 rounded-full" />
        <div className="bg-foreground/20 h-1 w-1/2 rounded-full" />
      </div>
    </div>
  );
}

/** What the sidebar leaves when it is closed, as two pictures to pick from. */
export function SidebarClosedChoice({
  label,
  value,
  onChange,
}: {
  label: string;
  value: SidebarCollapseMode;
  onChange: (mode: SidebarCollapseMode) => void;
}) {
  return (
    <ToggleGroupPrimitive.Root
      type="single"
      aria-label={label}
      value={value}
      // A choice is always made: picking the chosen one again keeps it.
      onValueChange={(next) => next && onChange(next as SidebarCollapseMode)}
      className="flex gap-3"
    >
      {CHOICES.map((choice) => (
        <ToggleGroupPrimitive.Item
          key={choice.mode}
          value={choice.mode}
          className={cn(
            "group text-foreground/60 hover:text-foreground data-[state=on]:text-foreground flex flex-col items-center gap-1.5 rounded-lg p-1 text-xs transition-colors",
            "[&>div:first-child]:transition-shadow data-[state=on]:[&>div:first-child]:ring-2 data-[state=on]:[&>div:first-child]:ring-ring",
          )}
        >
          <Picture strip={choice.mode === "strip"} />
          {choice.label}
        </ToggleGroupPrimitive.Item>
      ))}
    </ToggleGroupPrimitive.Root>
  );
}
