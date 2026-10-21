import { invoke } from "@tauri-apps/api/core";

import { useAction } from "@/app/conversation/useAction";
import { ErrorLine } from "@/components/controls";
import { Cursor } from "@/components/icons";

/** The helper app, to drag into the list in System Settings. */
export function HelperCard() {
  const drag = useAction();
  const reveal = useAction();
  return (
    <div className="flex flex-col gap-1.5">
      <div
        data-slot="helper-card"
        role="button"
        tabIndex={-1}
        aria-label="Brigadier Computer Use: drag this into the list in System Settings"
        onPointerDown={(event) => {
          if (event.button === 0) drag.run(() => invoke("computer_setup_drag"));
        }}
        className="border-divider bg-foreground/3 hover:bg-foreground/6 flex cursor-grab items-center gap-3 rounded-lg border p-3"
      >
        <span className="bg-foreground/10 flex size-10 shrink-0 items-center justify-center rounded-lg">
          <Cursor aria-hidden className="size-icon-lg" />
        </span>
        <div className="flex flex-col">
          <span className="font-medium">Brigadier Computer Use</span>
          <span className="text-foreground/60 text-xs">Drag this into the list in System Settings</span>
        </div>
      </div>
      <button
        type="button"
        className="text-foreground/60 hover:text-foreground self-start text-xs underline-offset-2 hover:underline"
        onClick={() => reveal.run(() => invoke("reveal_computer_helper"))}
      >
        Show in Finder
      </button>
      <ErrorLine error={drag.error ?? reveal.error} />
    </div>
  );
}
