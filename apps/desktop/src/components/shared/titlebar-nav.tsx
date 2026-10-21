import { useShortcuts } from "@/app/shortcuts";
import { TooltipIconButton } from "@/components/controls/tooltip-icon-button";
import { ArrowLeft } from "@/components/icons";
import { useSidebar } from "@/components/layout/sidebar";
import { SidebarToggle } from "@/components/shared/sidebar-toggle";
import { TitlebarTips } from "@/components/shared/titlebar-button";
import { stepHistory } from "@/state/actions";
import { useCanStep } from "@/state/history";

/** The buttons' glyphs dim to this while there is nowhere to go. */
const NOWHERE = "disabled:opacity-40";

/**
 * Back, Forward and the sidebar toggle, at a fixed spot in the titlebar on the traffic lights'
 * line: Back where the lights end on macOS, at the titlebar's start in full screen (where they
 * are gone) and elsewhere. They stay put while the sidebar collapses and expands.
 */
export function TitlebarNav() {
  const { open, toggleSidebar, canToggle } = useSidebar();
  const shortcuts = useShortcuts();
  const can = useCanStep();
  return (
    <div data-slot="titlebar-nav" className="h-titlebar-toggle start-titlebar-start absolute top-0 z-20 flex items-center gap-1.5">
      <TitlebarTips>
        <TooltipIconButton look="toolbar"
          tooltip="Back"
          shortcut={shortcuts.back}
          disabled={!can.back}
          className={NOWHERE}
          onClick={() => stepHistory(-1)}
        >
          <ArrowLeft />
        </TooltipIconButton>
        <TooltipIconButton look="toolbar"
          tooltip="Forward"
          shortcut={shortcuts.forward}
          disabled={!can.forward}
          className={NOWHERE}
          onClick={() => stepHistory(1)}
        >
          <ArrowLeft className="-scale-x-100" />
        </TooltipIconButton>
        {/* Settings holds the panel open, so it has no toggle; Back and Forward stay put. */}
        {canToggle && (
          <SidebarToggle open={open} onToggle={toggleSidebar} />
        )}
      </TitlebarTips>
    </div>
  );
}
