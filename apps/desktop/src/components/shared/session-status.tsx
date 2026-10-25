import { SidebarError as SidebarErrorGlyph, SidebarWorking } from "@/components/icons";
import { cn } from "@/lib/utils";
import { SESSION_STATUS_WORDS, type SessionStatus, useSessionStatus, useSidebarIndicators } from "@/state/tabStates";

/** "Working · watching CI": a state's words and its reason, on one line. */
export function statusWords({ state, reason }: SessionStatus): string {
  const words = SESSION_STATUS_WORDS[state];
  return reason ? `${words} · ${reason}` : words;
}

/** A sidebar row's state word with its activity or unread mark beside it; the tab shows only the marks. */
export function SessionStatusLabel({
  conversationId,
  className,
  rowIndicator,
}: {
  conversationId: string;
  className?: string;
  rowIndicator?: React.ReactNode;
}) {
  const status = useSessionStatus(conversationId);
  if (!status) return null;
  const { state } = status;
  return (
    <span
      data-slot="row-status"
      data-status={state}
      title={statusWords(status)}
      className={cn("text-muted-foreground flex max-w-2/5 min-w-0 shrink items-center gap-1 overflow-hidden text-2xs whitespace-nowrap", className)}
    >
      <span data-slot="status-word" className="shrink-0">{SESSION_STATUS_WORDS[state]}</span>
      {rowIndicator}
    </span>
  );
}

/** A 16-unit error glyph in the row's leading 20-unit slot. */
export function SidebarError({ conversationId }: { conversationId: string }) {
  const { error } = useSidebarIndicators(conversationId);
  if (!error) return null;
  return <span data-slot="status-error" role="img" aria-label="Failed" className="flex size-icon-lg shrink-0 items-center justify-center text-destructive">
    <SidebarErrorGlyph aria-hidden className="size-icon-md" />
  </span>;
}

/** The trailing 20-unit slot contains a spinner or a blue dot for Needs you or unread Done. */
export function SidebarActivityMark({ conversationId, unread }: { conversationId: string; unread: boolean }) {
  const mark = useSidebarIndicators(conversationId, unread);
  const needsYou = useSessionStatus(conversationId)?.state === "needsYou";
  if (!mark.working && !mark.unread) return null;
  return <span className="flex size-icon-lg shrink-0 items-center justify-center">
    {mark.working ? <SidebarWorking data-slot="status-spinner" role="status" aria-label="Working" className="text-foreground/70 size-icon-md motion-safe:animate-[spin_2s_steps(60,end)_infinite]" /> : <span data-slot="status-unread" role="img" aria-label={needsYou ? "Needs you" : "Unread"} className="bg-link size-2 shrink-0 rounded-full" />}
  </span>;
}
