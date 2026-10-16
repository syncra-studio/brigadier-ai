import { Settings } from "@openai/apps-sdk-ui/components/Icon";
import type { ReactNode } from "react";

import { KeepAwakeMenu, UsageMenu } from "@/app/BarStatus";
import { useShortcuts } from "@/app/shortcuts";
import { preloadSettings } from "@/app/settings/lazy";
import { FootButton } from "@/app/sidebar/nav";
import { RestartToUpdate } from "@/app/RestartToUpdate";
import { UpdatesRow } from "@/app/UpdatesRow";
import { useSidebar } from "@/components/ui/sidebar";
import { cn } from "@/lib/utils";
import { toggleSettings } from "@/state/actions";

/**
 * The icons at the sidebar's foot, outside Settings: keeping awake, the agents' usage, updates
 * while there are any, "Restart to update" while a nightly update waits, and Settings. Expanded,
 * they make one row along the bottom, Settings first; collapsed, they stack in the strip, Settings
 * last. Settings stays in the corner either way, and the others fade in where they move to, so the
 * foot doesn't jump as the sidebar collapses or expands. In a narrow row "Restart to update"
 * shortens its words.
 */
export function SidebarFoot() {
  const shortcuts = useShortcuts();
  const { open } = useSidebar();
  const settings = (
    <FootButton
      tip="Settings"
      shortcut={shortcuts.settings}
      icon={<Settings />}
      // Its code loads as the pointer comes (if idle hasn't loaded it yet), before the click.
      onPointerEnter={preloadSettings}
      onFocus={preloadSettings}
      onClick={toggleSettings}
    />
  );
  return (
    <div
      className={cn(
        "flex shrink-0 px-2 py-2",
        open ? "flex-row items-center gap-1" : "flex-col gap-px",
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
      <Moving open={open} shrink>
        <RestartToUpdate />
      </Moving>
      {!open && settings}
    </div>
  );
}

/** A foot icon that moves between the row and the stack: it fades in where it lands. */
function Moving({ open, shrink = false, children }: { open: boolean; shrink?: boolean; children: ReactNode }) {
  return (
    <div
      className={cn(
        "flex motion-reduce:animate-none",
        shrink && "min-w-0",
        // Two names for one fade, so it starts again on each turn.
        open ? "animate-[foot-row_150ms_ease-out]" : "animate-[foot-stack_150ms_ease-out]",
      )}
    >
      {children}
    </div>
  );
}
