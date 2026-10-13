import type { ReactNode } from "react";

import { AgentStatusIndicator, type AgentState } from "@/components/assistant-ui/elements/agent-status";
import { mono, paper } from "@/components/assistant-ui/elements/surfaces";
import { cn } from "@/lib/utils";

/** assistant-ui's background inbox, with room for each preview's controls and log. */
export function BackgroundInbox({ summary, actions, children }: { summary: string; actions: ReactNode; children: ReactNode }) {
  return <section data-slot="background-inbox" aria-label="Session previews" className={cn(paper, "flex min-h-0 flex-1 flex-col border-0 bg-transparent")}>
    <header className="space-y-2 px-3 py-2.5">
      <div className="flex items-baseline justify-between gap-3">
      <h2 className="text-label font-medium">Previews</h2>
      <span className={cn(mono, "text-muted-foreground text-end")}>{summary}</span>
      </div>
      <div className="flex items-center justify-end gap-1">{actions}</div>
    </header>
    <div className="min-h-0 flex-1 overflow-y-auto">{children}</div>
  </section>;
}

export function BackgroundInboxRow({ state, status, name, description, detail, elapsed, children }: {
  state: AgentState; status: string; name: string; description: string; detail: string;
  elapsed: string; children: ReactNode;
}) {
  return <article aria-label={name} className="border-border/50 border-t px-3 py-3">
    <div className="flex items-center gap-2">
      <AgentStatusIndicator state={state} />
      <h3 className="min-w-0 flex-1 truncate text-label font-medium" title={name}>{name}</h3>
      <span className={cn(mono, "text-muted-foreground shrink-0 tabular-nums")} title="Duration, including pauses">{elapsed}</span>
    </div>
    <p className={cn(mono, "text-muted-foreground mt-1 truncate")} title={detail}>
      <span className="sr-only">{status}. </span>{description}
    </p>
    {children}
  </article>;
}
