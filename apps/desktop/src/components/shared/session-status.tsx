import { SidebarError as SidebarErrorGlyph, SidebarWorking } from "@/components/icons";
import { useRowMark } from "@/state/tabStates";

/** A 16-unit error glyph in the row's leading 20-unit slot. */
export function SidebarError({ conversationId }: { conversationId: string }) {
  const { look, hover } = useRowMark(conversationId);
  if (look !== "error") return null;
  return <span data-slot="status-error" role="img" aria-label="Failed" title={hover ?? undefined} className="flex size-icon-lg shrink-0 items-center justify-center text-destructive">
    <SidebarErrorGlyph aria-hidden className="size-icon-md" />
  </span>;
}

/** The trailing 20-unit slot: a spinner while it works, a blue dot when it needs the user or has a new reply. */
export function SidebarActivityMark({ conversationId }: { conversationId: string }) {
  const { look, hover } = useRowMark(conversationId);
  if (look !== "working" && look !== "dot") return null;
  return <span className="flex size-icon-lg shrink-0 items-center justify-center">
    {look === "working"
      ? <SidebarWorking data-slot="status-spinner" role="status" aria-label={hover ?? "Working"} className="text-foreground/70 size-icon-md motion-safe:animate-[spin_2s_steps(60,end)_infinite]" />
      : <span data-slot="status-unread" role="img" aria-label={hover ?? "New reply"} className="bg-link size-2 shrink-0 rounded-full" />}
  </span>;
}
