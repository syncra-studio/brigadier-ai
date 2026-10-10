import { shortcutKeys } from "@/app/shortcutCatalog";
import { useApp } from "@/state/store";

/** The app's keyboard shortcuts as the tooltips and menus show them, for this platform. */
export function useShortcuts() {
  const mac = useApp((s) => s.info?.platform === "macos");
  return {
    mac,
    sidebar: shortcutKeys("sidebar", mac),
    back: shortcutKeys("back", mac),
    forward: shortcutKeys("forward", mac),
    search: shortcutKeys("search", mac),
    settings: shortcutKeys("settings", mac),
    terminal: shortcutKeys("terminal", mac),
    inspector: shortcutKeys("inspector", mac),
  };
}
