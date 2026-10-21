import { BarItem } from "@/app/shell/BarItem";
import { shortcutLabel } from "@/app/side-panel/shortcutLabel";
import { TooltipIconButton } from "@/components/controls";
import { Dock } from "@/components/icons";
import { placeOf, terminalWorksHere, toggleTerminal, useTerminalPlaces } from "@/state/terminalPlaces";
import { useApp } from "@/state/store";

/** The titlebar's Terminal toggle, where a terminal can work. */
export function TerminalButton() {
  const mac = useApp((s) => s.info?.platform === "macos");
  const works = useApp(terminalWorksHere);
  const place = useApp((s) => placeOf(s.selection));
  const open = useTerminalPlaces((s) => s.places[place]?.open ?? false);
  return (
    <BarItem show={works}>
      <TooltipIconButton look="toolbar"
        tooltip="Terminal"
        shortcut={shortcutLabel("⌘J", mac)}
        aria-pressed={open}
        onClick={() => toggleTerminal()}
      >
        <Dock />
      </TooltipIconButton>
    </BarItem>
  );
}
