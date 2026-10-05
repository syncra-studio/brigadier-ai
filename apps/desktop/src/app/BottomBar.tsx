import { PlusCircle, Settings, Terminal } from "@openai/apps-sdk-ui/components/Icon";

import { BarItem } from "@/app/BarItem";
import { KeepAwakeMenu, UsageMenu } from "@/app/BarStatus";
import { useShortcuts } from "@/app/shortcuts";
import { UpdatePills } from "@/app/UpdatePills";
import { BarButton } from "@/app/sidebar/nav";
import { toggleSettings } from "@/state/actions";
import { useBarActions } from "@/state/barActions";
import { useApp } from "@/state/store";
import {
  placeOf,
  terminalWorksHere,
  toggleTerminal,
  useTerminalPlaces,
} from "@/state/terminalPlaces";

/**
 * The bar along the window's bottom, on the window chrome under the sidebar panel and the
 * content alike, and always there: Settings, the agents' usage and keeping awake at its start;
 * what is only sometimes there (updates, Side chat) and the terminal at its end. The terminal
 * is there everywhere but in an archived thread, which can't start one.
 */
export function BottomBar() {
  const inSettings = useApp((s) => s.selection.type === "settings");
  const shortcuts = useShortcuts();
  const sideChat = useBarActions((s) => s.sideChat);
  const place = useApp((s) => placeOf(s.selection));
  const terminalWorks = useApp(terminalWorksHere);
  // On while it shows: full view can hide an open one.
  const terminalCovered = useBarActions((s) => s.terminalCover !== null);
  const terminalOn =
    useTerminalPlaces((s) => s.places[place]?.open ?? false) && !terminalCovered;
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
      <BarItem show={sideChat !== null}>
        <BarButton
          label="Side chat"
          shortcut={shortcuts.sideChat}
          selected={sideChat?.on ?? false}
          onClick={() => useBarActions.getState().sideChat?.toggle()}
        >
          <PlusCircle />
        </BarButton>
      </BarItem>
      <BarItem show={terminalWorks}>
        <BarButton
          label="Terminal"
          shortcut={shortcuts.terminal}
          selected={terminalOn}
          onClick={() => toggleTerminal()}
        >
          <Terminal />
        </BarButton>
      </BarItem>
    </footer>
  );
}
