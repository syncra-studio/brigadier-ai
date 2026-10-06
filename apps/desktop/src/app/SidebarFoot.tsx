import { Settings } from "@openai/apps-sdk-ui/components/Icon";

import { KeepAwakeMenu, UsageMenu } from "@/app/BarStatus";
import { useShortcuts } from "@/app/shortcuts";
import { FootRow } from "@/app/sidebar/nav";
import { UpdatesRow } from "@/app/UpdatesRow";
import { toggleSettings } from "@/state/actions";
import { useApp } from "@/state/store";

/**
 * The rows at the sidebar's foot, expanded or collapsed to the strip, and in Settings too:
 * keeping awake, the agents' usage, updates while there are any, and Settings (filled while it
 * is open, when it closes it).
 */
export function SidebarFoot() {
  const inSettings = useApp((s) => s.selection.type === "settings");
  const shortcuts = useShortcuts();
  return (
    <div className="flex shrink-0 flex-col gap-px px-2 pt-2 pb-2">
      <KeepAwakeMenu />
      <UsageMenu />
      <UpdatesRow />
      <FootRow
        label="Settings"
        tip={inSettings ? "Close settings" : "Settings"}
        shortcut={shortcuts.settings}
        selected={inSettings}
        icon={<Settings />}
        onClick={toggleSettings}
      />
    </div>
  );
}
