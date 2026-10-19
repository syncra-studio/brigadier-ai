import { Spinner } from "@/components/glyphs/spinner";
import { cn } from "@/lib/utils";
import { SESSION_STATUS_WORDS, type SessionStatus, useSessionStatus } from "@/state/tabStates";

/** "Working · watching CI": a state's words and its reason, on one line. */
export function statusWords({ state, reason }: SessionStatus): string {
  const words = SESSION_STATUS_WORDS[state];
  return reason ? `${words} · ${reason}` : words;
}

/** A sidebar row pairs its state word with a spinner or dot; a tab shows only the mark. */
export function SessionStatusLabel({
  conversationId,
  place,
  className,
  fallback = null,
}: {
  conversationId: string;
  place: "row" | "tab";
  className?: string;
  fallback?: React.ReactNode;
}) {
  const status = useSessionStatus(conversationId);
  if (!status) return fallback;
  const words = statusWords(status);
  const { state } = status;
  const busy = state === "working" || state === "merging";
  return (
    <span
      data-slot={place === "row" ? "row-status" : "tab-status"}
      data-status={state}
      title={words}
      className={cn(
        "flex items-center gap-1 text-2xs whitespace-nowrap",
        place === "row" ? "max-w-2/5 min-w-0 shrink overflow-hidden" : "shrink-0",
        state === "needsYou" && place === "row"
          ? "bg-success/15 text-success rounded-capsule h-pill px-pill-x shrink-0"
          : state === "ready" || state === "prOpen"
            ? "text-link"
            : state === "needsYou" ? "text-success" : state === "failed" ? "text-destructive" : "text-muted-foreground",
        place === "tab" && "opacity-80",
        className,
      )}
    >
      {busy
        ? <Spinner data-slot="status-spinner" aria-hidden className="size-icon-sm shrink-0 animate-spin motion-reduce:animate-none" />
        : <span data-slot="status-dot" aria-hidden className="bg-current size-1.5 shrink-0 rounded-full" />}
      {place === "row" && <span data-slot="status-word" className="shrink-0">{SESSION_STATUS_WORDS[state]}</span>}
    </span>
  );
}
