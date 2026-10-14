// Ported from assistant-ui’s standalone registry (MIT); see THIRD_PARTY_NOTICES.md.
// Icon imports and unavailable theme classes map to Brigadier equivalents; data-state exposes the state to fixtures.

import type { ComponentProps, ReactNode } from "react";
import { Check as CheckIcon, Pause as PauseIcon, Regenerate as RotateCcwIcon, X as XIcon } from "@openai/apps-sdk-ui/components/Icon";
import { cn } from "@/lib/utils";
import { mono, paper } from "./surfaces";

export type AgentState = "working" | "waiting" | "done" | "failed";

export interface StatusStep {
  state: AgentState;
  label: string;
}

export function AgentStatus({
  state,
  label,
  elapsed,
  trailing,
  className,
  ...props
}: Omit<ComponentProps<"span">, "children" | "state" | "label" | "elapsed"> & {
  state: AgentState;
  label: string;
  elapsed?: string | undefined;
  trailing?: ReactNode | undefined;
}) {
  return (
    <span
      data-slot="agent-status"
      data-state={state}
      className={cn(
        paper,
        "flex items-center gap-2.5 rounded-capsule py-1.5 ps-3.5 pe-1.5",
        className,
      )}
      {...props}
    >
      {state === "done" ? (
        <CheckIcon aria-hidden className="size-icon-xs shrink-0 text-success" />
      ) : state === "failed" ? (
        <XIcon aria-hidden className="text-destructive size-icon-xs shrink-0" />
      ) : (
        <span
          aria-hidden
          className={cn(
            "size-1.5 shrink-0 rounded-capsule motion-reduce:animate-none",
            state === "working"
              ? "animate-pulse bg-link"
              : "border-foreground/35 border",
          )}
        />
      )}
      <span className="sr-only">{state}</span>
      <span
        key={label}
        className="fade-in blur-in-2 animate-in min-w-0 max-w-44 truncate text-xs duration-300 motion-reduce:animate-none"
      >
        {label}
      </span>
      {elapsed !== undefined && state !== "done" && state !== "failed" && (
        <span className={cn(mono, "text-muted-foreground tabular-nums")}>
          {elapsed}
        </span>
      )}
      <span
        aria-hidden
        data-slot="agent-status-trailing"
        className="text-muted-foreground flex size-6 items-center justify-center rounded-capsule"
      >
        {trailing !== undefined ? (
          trailing
        ) : state === "done" || state === "failed" ? (
          <RotateCcwIcon className="size-icon-xs" />
        ) : (
          <PauseIcon className="size-icon-xs" />
        )}
      </span>
    </span>
  );
}
