import { SidebarError as SidebarErrorGlyph, SidebarQuestion, SidebarWorking } from "@/components/icons";
import { useRowMark } from "@/state/tabStates";

/** A 16-unit error glyph in the row's leading 20-unit slot. */
export function SidebarError({ conversationId }: { conversationId: string }) {
  const { look, hover } = useRowMark(conversationId);
  if (look !== "error") return null;
  return <span data-slot="status-error" role="img" aria-label="Failed" title={hover ?? undefined} className="flex size-icon-lg shrink-0 items-center justify-center text-destructive">
    <SidebarErrorGlyph aria-hidden className="size-icon-md" />
  </span>;
}

/** A sidebar row's trailing mark when it waits on the user (a question, approval or sign-in): a calm blue ring. */
export function SidebarWaitingMark({ label }: { label: string | null }) {
  return <span data-slot="status-waiting" role="img" aria-label={label ?? "Needs you"} className="flex size-icon-lg shrink-0 items-center justify-center text-link">
    <SidebarQuestion aria-hidden className="size-icon-md" />
  </span>;
}

/** A sidebar row's trailing mark when its run failed: an amber ring. */
export function SidebarAmberMark({ label }: { label: string | null }) {
  return <span data-slot="status-amber" role="img" aria-label={label ?? "Failed"} className="flex size-icon-lg shrink-0 items-center justify-center text-warning">
    {/* Turned over, its ring's i reads as an exclamation. */}
    <SidebarErrorGlyph aria-hidden className="size-icon-md rotate-180" />
  </span>;
}

/**
 * The trailing 20-unit slot: a spinner while it works, a blue dot for a new reply. On a chat tab the
 * dot also means it waits on the user; a sidebar row has {@link SidebarWaitingMark} for that.
 */
export function SidebarActivityMark({ conversationId }: { conversationId: string }) {
  const { look, hover } = useRowMark(conversationId);
  if (look !== "working" && look !== "dot") return null;
  return <span className="flex size-icon-lg shrink-0 items-center justify-center">
    {look === "working"
      ? <SidebarWorking data-slot="status-spinner" role="status" aria-label={hover ?? "Working"} className="text-foreground/70 size-icon-md motion-safe:animate-[spin_2s_steps(60,end)_infinite]" />
      : <span data-slot="status-unread" role="img" aria-label={hover ?? "New reply"} className="bg-link size-2 shrink-0 rounded-full" />}
  </span>;
}

/** A sidebar row's one mark: the blue ring, the amber ring, a spinner, a dot or nothing. */
export function SidebarRowMark({ conversationId }: { conversationId: string }) {
  const { hover, waiting, amber } = useRowMark(conversationId);
  if (waiting) return <SidebarWaitingMark label={hover} />;
  if (amber) return <SidebarAmberMark label={hover} />;
  return <SidebarActivityMark conversationId={conversationId} />;
}
