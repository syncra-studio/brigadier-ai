import { ChevronRight, Clock } from "@openai/apps-sdk-ui/components/Icon";

import { thoughtTopic } from "@/app/conversation/activity/group";
import { CHEVRON, ROW, ROW_TOGGLE } from "@/components/assistant-ui/elements/activity-row";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { cn } from "@/lib/utils";
import { formatDuration } from "@/lib/format";

/**
 * Thinking, in either thread. Live, it is the turn's live line, one line: the thought's newest
 * heading, else its first sentence, else "Thinking", shimmering. Settled, it is a row inside a
 * work group that says what it was about, "Thought for 4s · Checking the tests" (no time when the
 * provider's own events didn't measure one), and opens to the thought. A thought with no words
 * has no row. Token ticks never announce a live region.
 */
export function ThinkingRow({ text, ms = 0, live }: {
  text: string;
  /** How long the provider reasoned, measured from its own events; 0 when unknown. */
  ms?: number;
  live: boolean;
}) {
  if (live) {
    return (
      <div data-slot="thinking-live" className="shimmer w-fit max-w-full truncate text-sm leading-(--spacing-activity-row) motion-reduce:animate-none">
        {thoughtTopic(text, "newest") || "Thinking"}
      </div>
    );
  }
  if (!text.trim()) return null;
  const topic = thoughtTopic(text);
  return (
    <Collapsible data-slot="thinking-settled">
      <CollapsibleTrigger className={cn(ROW, ROW_TOGGLE)}>
        <Clock aria-hidden className="size-4 shrink-0" />
        <span className="shrink-0">{ms < 1000 ? "Thought" : `Thought for ${formatDuration(ms)}`}</span>
        {topic && <span className="text-foreground/40 group-hover:text-foreground/60 min-w-0 truncate">· {topic}</span>}
        <ChevronRight aria-hidden className={CHEVRON} />
      </CollapsibleTrigger>
      <CollapsibleContent className="text-foreground/60 thinking-fade max-h-thought overflow-y-auto ps-5.5 pt-1 pb-1 text-sm whitespace-pre-wrap wrap-anywhere">
        {text}
      </CollapsibleContent>
    </Collapsible>
  );
}
