import { ChevronRight, Clock } from "@openai/apps-sdk-ui/components/Icon";

import { CHEVRON, ROW } from "@/components/assistant-ui/elements/activity-row";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { cn } from "@/lib/utils";
import { formatDuration } from "@/lib/format";

/** The newest two lines of a streamed summary; the complete text stays in the settled row. */
export function thinkingSnippet(text: string): string {
  return text.trim().split("\n").filter((line) => line.trim()).slice(-2).join("\n").slice(-320);
}

/**
 * Thinking, in either thread. Live, it is the turn's live line: "Thinking", then the newest lines
 * of the thought. Settled, it is a row inside a work group, "Thought for 4s", that opens to the
 * thought. Token ticks never announce a live region.
 */
export function ThinkingRow({ text, startedAtMs, endedAtMs, live }: {
  text: string;
  startedAtMs: number;
  endedAtMs: number;
  live: boolean;
}) {
  if (live) {
    if (!text.trim()) return <div data-slot="thinking-live" className="shimmer w-fit text-sm motion-reduce:animate-none">Thinking</div>;
    return (
      <div data-slot="thinking-live" className="text-foreground/50 relative flex max-h-10 items-end overflow-hidden text-sm leading-5 whitespace-pre-wrap wrap-anywhere thinking-snippet">
        <span className="w-full shrink-0">{thinkingSnippet(text)}</span>
      </div>
    );
  }
  if (!text.trim()) return null;
  const elapsed = Math.max(0, endedAtMs - startedAtMs);
  return (
    <Collapsible data-slot="thinking-settled">
      <CollapsibleTrigger className={cn(ROW, "group hover:text-foreground rounded-control w-full text-start")}>
        <Clock aria-hidden className="size-4 shrink-0" />
        <span>{elapsed < 1000 ? "Thought" : `Thought for ${formatDuration(elapsed)}`}</span>
        <ChevronRight aria-hidden className={CHEVRON} />
      </CollapsibleTrigger>
      <CollapsibleContent className="text-foreground/50 thinking-fade max-h-35 overflow-y-auto ps-5.5 pt-1 pb-1 text-sm whitespace-pre-wrap wrap-anywhere">
        {text}
      </CollapsibleContent>
    </Collapsible>
  );
}
