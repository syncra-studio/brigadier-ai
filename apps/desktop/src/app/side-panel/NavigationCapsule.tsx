import { ArrowLeft, ArrowRight, Reload, Spinner } from "@/components/icons";
import { TooltipIconButton } from "@/components/controls";
import type { browserGo } from "@/ipc/client";

/** Back, Forward and Reload: round, and faint while they can't act. */
const NAV_BUTTON = "rounded-full disabled:opacity-40 disabled:text-muted-foreground";

/** The browser toolbar's Back, Forward and Reload (Stop while the page loads), in one capsule. */
export function NavigationCapsule({ disabled, loading, onAct }: {
  disabled: boolean;
  loading: boolean;
  onAct: (action: Parameters<typeof browserGo>[1]) => void;
}) {
  return (
    <div role="group" aria-label="Navigation" data-slot="browser-navigation" className="bg-muted/60 flex h-control-md shrink-0 items-center rounded-full p-0.5">
      <TooltipIconButton tooltip="Back" size="icon-md" className={NAV_BUTTON} disabled={disabled} onClick={() => onAct("back")}>
        <ArrowLeft />
      </TooltipIconButton>
      <TooltipIconButton tooltip="Forward" size="icon-md" className={NAV_BUTTON} disabled={disabled} onClick={() => onAct("forward")}>
        <ArrowRight />
      </TooltipIconButton>
      <span aria-hidden className="bg-border mx-1 h-4 w-px shrink-0" />
      {/* While the page loads the spinner stands in for Reload, and stops the load. */}
      <TooltipIconButton
        tooltip={loading ? "Stop loading" : "Reload"}
        size="icon-md"
        className={NAV_BUTTON}
        aria-busy={loading}
        disabled={disabled}
        onClick={() => onAct(loading ? "stop" : "reload")}
      >
        {loading ? <Spinner className="animate-spin" /> : <Reload />}
      </TooltipIconButton>
    </div>
  );
}
