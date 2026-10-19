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
  fallback = null,
  rowIndicator,
}: {
  conversationId: string;
  className?: string;
  fallback?: React.ReactNode;
  rowIndicator?: React.ReactNode;
}) {
  const status = useSessionStatus(conversationId);
  if (!status) return fallback;
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
    <svg aria-hidden viewBox="0 0 16 16" fill="currentColor" className="size-icon-md">
      <path d="M8 7.48633C8.28351 7.48633 8.51367 7.71649 8.51367 8V10.667C8.5135 10.9503 8.2834 11.1797 8 11.1797C7.7166 11.1797 7.4865 10.9503 7.48633 10.667V8C7.48633 7.71649 7.71649 7.48633 8 7.48633Z" />
      <path d="M8 4.90137C8.37814 4.9015 8.68544 5.20779 8.68555 5.58594C8.68528 5.96395 8.37804 6.27135 8 6.27148C7.62194 6.27138 7.31569 5.96396 7.31543 5.58594C7.31554 5.20778 7.62184 4.90147 8 4.90137Z" />
      <path fillRule="evenodd" clipRule="evenodd" d="M8 1.48633C11.5972 1.48633 14.5137 4.40279 14.5137 8C14.5137 11.5972 11.5972 14.5137 8 14.5137C4.40279 14.5137 1.48633 11.5972 1.48633 8C1.48633 4.40279 4.40279 1.48633 8 1.48633ZM8 2.51367C4.9698 2.51367 2.51367 4.9698 2.51367 8C2.51367 11.0302 4.9698 13.4863 8 13.4863C11.0302 13.4863 13.4863 11.0302 13.4863 8C13.4863 4.9698 11.0302 2.51367 8 2.51367Z" />
    </svg>
  </span>;
}

/** The trailing 20-unit slot contains a running spinner or an unread dot, otherwise nothing. */
export function SidebarActivityMark({ conversationId, unread }: { conversationId: string; unread: boolean }) {
  const mark = useSidebarIndicators(conversationId, unread);
  if (!mark.working && !mark.unread) return null;
  return <span className="flex size-icon-lg shrink-0 items-center justify-center">
    {mark.working ? <svg data-slot="status-spinner" role="status" aria-label="Working" viewBox="0 0 24 24" fill="currentColor" className="text-foreground/70 size-icon-md motion-safe:animate-[spin_2s_steps(60,end)_infinite]">
      <path opacity=".3" d="M18 12C18 8.68629 15.3137 6 12 6C8.68629 6 6 8.68629 6 12C6 15.3137 8.68629 18 12 18C15.3137 18 18 15.3137 18 12ZM20 12C20 16.4183 16.4183 20 12 20C7.58172 20 4 16.4183 4 12C4 7.58172 7.58172 4 12 4C16.4183 4 20 7.58172 20 12Z" />
      <path d="M12 4C16.4183 4 20 7.58172 20 12C20 16.4183 16.4183 20 12 20C7.58172 20 4 16.4183 4 12H6C6 15.3137 8.68629 18 12 18C15.3137 18 18 15.3137 18 12C18 8.68629 15.3137 6 12 6V4Z" />
    </svg> : <span data-slot="status-unread" role="img" aria-label="Unread" className="bg-link size-2 shrink-0 rounded-full" />}
  </span>;
}
