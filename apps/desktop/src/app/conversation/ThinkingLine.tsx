import { useCalmLabel } from "@/app/conversation/activity/calmLabel";
import { thinkingLabel } from "@/app/conversation/activity/group";

/**
 * Thinking, in either thread: only ever the turn's live line, one shimmering line with the
 * thought's newest heading or last whole line, else "Thinking"; it changes at most once a second.
 * A thought is never a row of the thread. Token ticks never announce a live region.
 */
export function ThinkingLine({ text }: { text: string }) {
  const { shown, pending } = useCalmLabel(thinkingLabel(text) || "Thinking", true);
  return (
    <div
      data-slot="thinking-live"
      data-pending={pending || undefined}
      className="shimmer w-fit max-w-full truncate text-sm leading-(--spacing-activity-row) motion-reduce:animate-none"
    >
      {shown}
    </div>
  );
}
