import { Check, ChevronDown, X } from "@openai/apps-sdk-ui/components/Icon";

import { fieldInteractive, mono } from "@/components/assistant-ui/elements/surfaces";
import { Spinner } from "@/components/glyphs/spinner";
import { cn } from "@/lib/utils";

export type AgentState = "starting" | "running" | "paused" | "done" | "failed" | "stopped";

/** Shape and motion carry status as well as colour; the caller supplies its accessible label. */
export function AgentStatusIndicator({ state, pill = false }: { state: AgentState; pill?: boolean }) {
  return <span aria-hidden data-slot="agent-status-indicator" data-state={state} className="flex size-icon-sm shrink-0 items-center justify-center">
    {state === "done" || state === "stopped" ? <Check className={cn("size-icon-sm", state === "done" ? "text-success" : "text-muted-foreground")} />
      : state === "failed" ? <X className="text-destructive size-icon-sm" />
      : state === "starting" && !pill ? <Spinner className="text-success size-icon-sm motion-safe:animate-spin" />
      : <span className={cn("size-1.5 rounded-full", state === "paused"
        ? "border-muted-foreground border"
        : "bg-success", pill && state !== "paused" && "motion-safe:animate-pulse")} />}
  </span>;
}

/** assistant-ui's agent-status pill, using Brigadier's surfaces and density tokens. */
export function AgentStatus({ state, label, status, elapsed }: { state: AgentState; label: string; status: string; elapsed?: string | undefined }) {
  return <span data-slot="agent-status" className={cn(fieldInteractive, "h-pill rounded-capsule inline-flex max-w-full items-center gap-2 px-2.5 text-xs")}>
    <AgentStatusIndicator state={state} pill />
    <span className="sr-only">{status}: </span>
    <span className="min-w-0 truncate">{label}</span>
    {elapsed && <span className={cn(mono, "text-muted-foreground shrink-0 tabular-nums")}>{elapsed}</span>}
    <ChevronDown aria-hidden className="text-muted-foreground size-icon-xs shrink-0" />
  </span>;
}
