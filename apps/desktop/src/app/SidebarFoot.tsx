import { Settings } from "@openai/apps-sdk-ui/components/Icon";
import type { ReactNode } from "react";

import { KeepAwakeMenu, UsageMenu } from "@/app/BarStatus";
import { useShortcuts } from "@/app/shortcuts";
import { FootButton } from "@/app/sidebar/nav";
import { UpdatesRow } from "@/app/UpdatesRow";
import { useSidebar } from "@/components/ui/sidebar";
import { cn } from "@/lib/utils";
import { toggleSettings } from "@/state/actions";
import { useApp } from "@/state/store";

/**
 * The icons at the sidebar's foot, in Settings too: keeping awake, the agents' usage, updates
 * while there are any, and Settings (filled while it is open, when it closes it). Expanded,
 * they make one row along the bottom, Settings first; collapsed, they stack in the strip,
 * Settings last. Settings stays in the corner either way, and the others fade in where they
 * move to, so the foot doesn't jump as the sidebar collapses or expands.
 */
export function SidebarFoot() {
  const inSettings = useApp((s) => s.selection.type === "settings");
  const shortcuts = useShortcuts();
  const { open } = useSidebar();
  const settings = (
    <FootButton
      tip={inSettings ? "Close settings" : "Settings"}
      shortcut={shortcuts.settings}
      selected={inSettings}
      icon={<Settings />}
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
      {!open && settings}
    </div>
  );
}

/** A foot icon that moves between the row and the stack: it fades in where it lands. */
function Moving({ open, children }: { open: boolean; children: ReactNode }) {
  return (
    <div
      className={cn(
        "flex motion-reduce:animate-none",
        // Two names for one fade, so it starts again on each turn.
        open ? "animate-[foot-row_150ms_ease-out]" : "animate-[foot-stack_150ms_ease-out]",
      )}
    >
      {children}
    </div>
  );
}
