import { type ReactNode, useRef } from "react";

import { TooltipIconButton } from "@/components/controls";
import { AppUpdatePill } from "@/app/shell/AppUpdatePill";
import { KeepAwakeMenu, UsageMenu } from "@/app/shell/BarStatus";
import { useShortcuts } from "@/app/shortcuts";
import { preloadSettings } from "@/app/settings/lazy";
import { footMenuPlacement } from "@/app/shell/sidebar/nav";
import { UpdatesRow } from "@/app/shell/UpdatesRow";
import { Comment, HelpIcon, Keyboard, Settings } from "@/components/icons";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/overlays";
import { useSidebar } from "@/components/layout";
import { cn } from "@/lib/utils";
import { openSettings, toggleSettings } from "@/state/actions";
import { openFeedback } from "@/state/feedback";

/**
 * The icons at the sidebar's foot, outside Settings: keeping awake, the agents' usage, updates
 * while there are any, a nightly update of Brigadier while there is one, Help, and Settings.
 * Expanded, they make one row along the bottom, Settings first and Help flush at its far right;
 * collapsed, they stack in the strip, Settings last. Settings stays in the corner either way, and
 * the others fade in where they move to, so the foot doesn't jump as the sidebar collapses or
 * expands. A row too narrow for every icon goes on to a second line.
 */
export function SidebarFoot() {
  const shortcuts = useShortcuts();
  const { open } = useSidebar();
  const settings = (
    <TooltipIconButton
      look="strip"
      side={open ? "top" : "right"}
      tooltip="Settings"
      shortcut={shortcuts.settings}
      // Its code loads as the pointer comes (if idle hasn't loaded it yet), before the click.
      onPointerEnter={preloadSettings}
      onFocus={preloadSettings}
      onClick={toggleSettings}
    >
      <Settings />
    </TooltipIconButton>
  );
  return (
    <div
      className={cn(
        "flex shrink-0 px-2 py-2",
        // A row too narrow for them all goes on to a second line, rather than past the edge.
        open ? "flex-row flex-wrap items-center gap-1" : "flex-col gap-px",
      )}
    >
      {open && settings}
      <Moving open={open}>
        <KeepAwakeMenu />
      </Moving>
      <Moving open={open}>
        <UsageMenu />
      </Moving>
      <Moving open={open}>
        <UpdatesRow />
      </Moving>
      <Moving open={open}>
        <AppUpdatePill />
      </Moving>
      <Moving open={open} end>
        <HelpMenu />
      </Moving>
      {!open && settings}
    </div>
  );
}

/** A foot icon that moves between the row and the stack: it fades in where it lands. */
function Moving({
  open,
  end = false,
  children,
}: {
  open: boolean;
  /** Flush at the row's far right. */
  end?: boolean;
  children: ReactNode;
}) {
  return (
    <div
      className={cn(
        "flex motion-reduce:animate-none",
        open && end && "ms-auto",
        // Two names for one fade, so it starts again on each turn.
        open ? "animate-[foot-row_150ms_ease-out]" : "animate-[foot-stack_150ms_ease-out]",
      )}
    >
      {children}
    </div>
  );
}

/** Help at the sidebar's foot: a life ring that opens Send feedback… and Keyboard shortcuts. */
function HelpMenu() {
  const { open } = useSidebar();
  // Send feedback opens once the menu has closed, so its note keeps the keyboard rather than the
  // closing menu handing it back to Help.
  const toFeedback = useRef(false);
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <TooltipIconButton look="strip" side={open ? "top" : "right"} tooltip="Help">
          <HelpIcon />
        </TooltipIconButton>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        {...(open ? ({ side: "top", align: "end" } as const) : footMenuPlacement(false))}
        // The design's width: sized to its two rows it read as an oval rather than a menu.
        className="w-60"
        onCloseAutoFocus={(event) => {
          if (!toFeedback.current) return;
          toFeedback.current = false;
          event.preventDefault();
          openFeedback();
        }}
      >
        <DropdownMenuItem
          onSelect={() => {
            toFeedback.current = true;
          }}
        >
          <Comment />
          Send feedback…
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={() => openSettings("shortcuts")}>
          <Keyboard />
          Keyboard shortcuts
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
