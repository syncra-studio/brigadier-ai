import type { ComponentPropsWithRef } from "react";

import { TooltipIconButton } from "@/components/controls/tooltip-icon-button";
import { ArrowUp, ArrowUpRight, PlayTriangle, Stop } from "@/components/icons";
import { cn } from "@/lib/utils";

/** What the composer's send button does now; see SendControls. */
export type SendState = "send" | "stop" | "resume";

/**
 * The composer's send button, a circle: ↑ to send (↗ when the message starts a new session in
 * the background), ■ to stop (including while starting or Esc is armed), ▶ to resume.
 * Its tip, disabled state and click come from where it sits.
 */
export function SendButton({
  state,
  background,
  className,
  ...props
}: Omit<ComponentPropsWithRef<typeof TooltipIconButton>, "children"> & { state: SendState; background: boolean }) {
  return (
    <TooltipIconButton
      {...props}
      side="bottom"
      type="button"
      variant="default"
      size="icon-md"
      data-state={state}
      className={cn("aui-composer-send rounded-capsule", className)}
    >
      <span className="flex items-center">
        {state === "stop" ? (
          <Stop className="size-icon-sm" />
        ) : state === "resume" ? (
          <PlayTriangle />
        ) : background ? (
          <ArrowUpRight data-slot="send-new-session" />
        ) : (
          <ArrowUp />
        )}
      </span>
    </TooltipIconButton>
  );
}
