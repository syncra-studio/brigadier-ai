import { Settings } from "@openai/apps-sdk-ui/components/Icon";

import { KeepAwakeMenu, UsageMenu } from "@/app/BarStatus";
import { useShortcuts } from "@/app/shortcuts";
import { UpdatePills } from "@/app/UpdatePills";
import { BarButton } from "@/app/sidebar/nav";
import { toggleSettings } from "@/state/actions";
import { useApp } from "@/state/store";

/**
 * The bar along the window's bottom, on the window chrome under the sidebar panel and the
 * content alike, and always there: Settings, the agents' usage and keeping awake at its start;
 * updates, while there are any, at its end.
 */
export function BottomBar() {
  const inSettings = useApp((s) => s.selection.type === "settings");
  const shortcuts = useShortcuts();
  return (
    <footer
      aria-label="App bar"
      data-tauri-drag-region
      className="h-bottom-bar bg-chrome px-surface-inset flex shrink-0 items-center gap-0.5"
    >
      <BarButton
        label={inSettings ? "Close settings" : "Settings"}
        shortcut={shortcuts.settings}
        selected={inSettings}
        onClick={toggleSettings}
      >
        <Settings />
      </BarButton>
      <UsageMenu />
      <KeepAwakeMenu />
      <div data-tauri-drag-region className="min-w-0 flex-1 self-stretch" />
      <UpdatePills />
    </footer>
  );
}
