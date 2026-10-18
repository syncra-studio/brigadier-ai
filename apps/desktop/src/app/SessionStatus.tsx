import { Spinner } from "@/components/glyphs/spinner";
import { cn } from "@/lib/utils";
import { TAB_STATE_WORDS, type SessionStatus, useSessionStatus } from "@/state/tabStates";

/** "Working · watching CI": a state's words and its reason, on one line. */
export function statusWords({ state, reason }: SessionStatus): string {
  const words = TAB_STATE_WORDS[state].one;
  return reason ? `${words} · ${reason}` : words;
}

/**
 * A session's state with its reason, on one line ("Working · watching CI", "Needs you ·
 * approval", "Ready · PR open"): a green pill while it needs the user, a spinner while it works
 * or merges, plain words once settled. `fallback` shows when it has no state. On a sidebar row
 * (`row`) it hides while the row's actions show; on the chat tab it is smaller and quieter. It
 * takes at most two fifths of the row or tab, and the reason is cut before the state: the title
 * is what tells sessions apart.
 */
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
        "flex max-w-2/5 min-w-0 shrink items-center gap-1 overflow-hidden text-2xs whitespace-nowrap",
        state === "needsYou"
          ? "bg-success/15 text-success rounded-capsule h-pill px-pill-x shrink-0"
          : state === "ready"
            ? "text-link"
            : "text-muted-foreground",
        place === "tab" && "opacity-80",
        className,
      )}
    >
      {busy && <Spinner aria-hidden className="size-icon-sm shrink-0 animate-spin motion-reduce:animate-none" />}
      <span className="shrink-0">{TAB_STATE_WORDS[state].one}</span>
      {/* Only the reason gives way: a weighted shrink isn't kept to in WebKit. */}
      {status.reason && <span className="min-w-0 overflow-hidden text-ellipsis"> · {status.reason}</span>}
    </span>
  );
}
