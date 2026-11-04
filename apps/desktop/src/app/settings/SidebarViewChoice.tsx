import { Segment, SegmentedRoot } from "@/components/controls";
import { errorText } from "@/app/shell/dialogs/fields";
import type { SidebarMode } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { setSetting } from "@/state/settings";
import { useApp } from "@/state/store";
import { toast } from "@/state/toasts";

const CHOICES: { mode: SidebarMode; label: string }[] = [
  { mode: "inbox", label: "Inbox" },
  { mode: "projects", label: "Projects" },
];

/** Each line of the picture's sidebar, as a share of its width; a project mark leads every third. */
const LINES = ["w-full", "w-3/4", "w-full", "w-2/3", "w-full", "w-3/4"];

/** A tiny window: the sidebar's rows in one list, or indented under a project mark each. */
function SidebarViewPicture({ grouped }: { grouped: boolean }) {
  return (
    <div className="border-divider bg-background flex h-14 w-24 gap-1 rounded-md border p-1">
      <div className="bg-foreground/15 flex w-11 shrink-0 flex-col justify-center gap-1 rounded-sm px-1">
        {LINES.map((width, index) => (
          <div key={index} className="flex items-center gap-0.5">
            {grouped && <div className={cn("size-1 shrink-0 rounded-xs", index % 3 === 0 ? "bg-foreground/70" : "bg-transparent")} />}
            <div className="min-w-0 flex-1"><div className={cn("bg-foreground/45 h-0.5 rounded-full", width)} /></div>
          </div>
        ))}
      </div>
      <div className="bg-foreground/5 flex flex-1 flex-col justify-end gap-1 rounded-sm p-1.5">
        <div className="bg-foreground/20 h-1 w-3/4 rounded-full" />
        <div className="bg-foreground/20 h-1 w-1/2 rounded-full" />
      </div>
    </div>
  );
}

/** How the sidebar lists, as two pictures to pick from: the same setting as its header's toggle. */
export function SidebarViewChoice({ label }: { label: string }) {
  const value = useApp((s) => (s.settings.sidebarMode === "projects" ? "projects" : "inbox"));
  return (
    <SegmentedRoot
      type="single"
      aria-label={label}
      value={value}
      // A choice is always made: picking the chosen one again keeps it.
      onValueChange={(next) => {
        if (next) void setSetting("sidebarMode", next as SidebarMode).catch((error: unknown) => toast(errorText(error), { tone: "error" }));
      }}
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
          <SidebarViewPicture grouped={choice.mode === "projects"} />
          {choice.label}
        </Segment>
      ))}
    </SegmentedRoot>
  );
}
