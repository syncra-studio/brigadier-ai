import { Check, ChevronDown, Sidebar, SidebarFloatingLeft } from "@openai/apps-sdk-ui/components/Icon";

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { tokenPx } from "@/lib/tokens";
import { cn } from "@/lib/utils";
import type { SidebarCollapseMode } from "@/state/sidebar";

/** One part of the shape: no wash or ring of its own, its glyph full while the keyboard is on it. */
const PART = cn(
  "flex h-full shrink-0 items-center justify-center rounded-toolbar-button outline-none",
  "[&_svg]:pointer-events-none [&_svg]:shrink-0",
  "not-in-data-[input=pointer]:focus-visible:text-foreground",
);

/**
 * The sidebar toggle in the titlebar: one icon button shape holding the sidebar glyph, which
 * shows or hides the sidebar, and a small chevron after it, which opens the choice of how it
 * hides (to its strip, or completely). The two parts share one wash on hover, while held and
 * while the choice is open, and one focus ring (for the keyboard only).
 */
export function SidebarToggle({
  open,
  onToggle,
  mode,
  onModeChange,
}: {
  open: boolean;
  onToggle: () => void;
  mode: SidebarCollapseMode;
  onModeChange: (mode: SidebarCollapseMode) => void;
}) {
  return (
    <div
      data-slot="sidebar-toggle"
      className={cn(
        "h-icon-button-md rounded-toolbar-button text-toolbar-foreground flex items-center transition-colors",
        "hover:bg-toolbar-hover hover:text-foreground has-active:bg-toolbar-active",
        "has-data-[state=open]:bg-toolbar-hover has-data-[state=open]:text-foreground",
        "not-in-data-[input=pointer]:has-focus-visible:outline-ring not-in-data-[input=pointer]:has-focus-visible:outline not-in-data-[input=pointer]:has-focus-visible:-outline-offset-1",
      )}
    >
      <button
        type="button"
        aria-label={open ? "Hide sidebar" : "Show sidebar"}
        aria-expanded={open}
        className={cn(PART, "w-icon-button-md [&_svg]:size-icon-md")}
        onClick={onToggle}
      >
        {open ? <Sidebar /> : <SidebarFloatingLeft />}
      </button>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button
            type="button"
            aria-label="How the sidebar hides"
            className={cn(PART, "w-titlebar-chevron [&_svg]:size-icon-xs")}
          >
            <ChevronDown />
          </button>
        </DropdownMenuTrigger>
        {/* Lined up with the shape's start, not the chevron's. */}
        <SidebarModeMenu mode={mode} onModeChange={onModeChange} />
      </DropdownMenu>
    </div>
  );
}

function SidebarModeMenu({
  mode,
  onModeChange,
}: {
  mode: SidebarCollapseMode;
  onModeChange: (mode: SidebarCollapseMode) => void;
}) {
  return (
    <DropdownMenuContent
      align="start"
      alignOffset={-tokenPx("--spacing-icon-button-md")}
    >
      <DropdownMenuRadioGroup
        value={mode}
        onValueChange={(value) => {
          if (value === "strip" || value === "hidden") onModeChange(value);
        }}
      >
        <DropdownMenuRadioItem value="strip" indicator={<Check />}>
          Collapse to strip
        </DropdownMenuRadioItem>
        <DropdownMenuRadioItem value="hidden" indicator={<Check />}>
          Hide completely
        </DropdownMenuRadioItem>
      </DropdownMenuRadioGroup>
    </DropdownMenuContent>
  );
}
