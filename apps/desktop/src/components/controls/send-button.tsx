import type { ComponentPropsWithRef } from "react";

import { TooltipIconButton } from "@/components/controls/tooltip-icon-button";
import { ArrowUp, ArrowUpRight, PlayTriangle, Spinner, Stop } from "@/components/icons";
import { cn } from "@/lib/utils";

/** What the composer's send button does now; see SendControls. */
export type SendState = "starting" | "armed" | "stop" | "queue" | "resume" | "send";

/**
 * The composer's send button, a circle: ↑ to send (↗ when the message starts a new session in
 * the background), ■ to stop, "Esc" once armed, ▶ to resume, a grey spinner while the model
 * starts. Its tip, whether it is disabled and its click come from where it sits.
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
      variant={state === "starting" ? "secondary" : "default"}
      size="icon-md"
      data-state={state}
      className={cn("aui-composer-send rounded-capsule", state === "armed" && "w-auto px-2 text-xs", className)}
    >
      {/* Keyed by state, so each glyph eases in as the button changes. */}
      <span key={background ? "background" : state} className="animate-in fade-in zoom-in-75 flex items-center duration-200 motion-reduce:animate-none">
        {state === "starting" ? (
          <Spinner className="size-icon-sm animate-spin motion-reduce:animate-none" />
        ) : state === "armed" ? (
          "Esc"
        ) : state === "stop" ? (
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
