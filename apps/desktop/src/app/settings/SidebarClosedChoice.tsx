import { SidebarClosedPicture } from "@/app/settings/SidebarClosedPicture";
import { Segment, SegmentedRoot } from "@/components/controls";
import { cn } from "@/lib/utils";
import type { SidebarCollapseMode } from "@/state/sidebar";

const CHOICES: { mode: SidebarCollapseMode; label: string }[] = [
  { mode: "strip", label: "Show icon strip" },
  { mode: "hidden", label: "Hide it completely" },
];

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
    <SegmentedRoot
      type="single"
      aria-label={label}
      value={value}
      // A choice is always made: picking the chosen one again keeps it.
      onValueChange={(next) => next && onChange(next as SidebarCollapseMode)}
      className="flex gap-3"
    >
      {CHOICES.map((choice) => (
        <Segment
          key={choice.mode}
          value={choice.mode}
          className={cn(
            "group text-foreground/60 hover:text-foreground data-[state=on]:text-foreground flex flex-col items-center gap-1.5 rounded-lg p-1 text-xs transition-colors",
            "[&>div:first-child]:transition-shadow data-[state=on]:[&>div:first-child]:ring-2 data-[state=on]:[&>div:first-child]:ring-ring",
          )}
        >
          <SidebarClosedPicture strip={choice.mode === "strip"} />
          {choice.label}
        </Segment>
      ))}
    </SegmentedRoot>
  );
}
