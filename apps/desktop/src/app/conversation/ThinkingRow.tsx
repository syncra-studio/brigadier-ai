import { ChevronRight, Clock } from "@openai/apps-sdk-ui/components/Icon";

import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { formatDuration } from "@/lib/format";

/** The newest two lines of a streamed summary; the complete text stays in the settled row. */
export function thinkingSnippet(text: string): string {
  return text.trim().split("\n").filter((line) => line.trim()).slice(-2).join("\n").slice(-320);
}

/** Shared by the main turn and a worker's thread. Token ticks never announce a live region. */
export function ThinkingRow({ text, startedAtMs, endedAtMs, live }: {
  text: string;
  startedAtMs: number;
  endedAtMs: number;
  live: boolean;
}) {
  if (!text.trim()) return null;
  if (live) {
    return (
      <div data-slot="thinking-live" className="text-muted-foreground relative flex max-h-10 items-end overflow-hidden text-sm leading-5 whitespace-pre-wrap wrap-anywhere thinking-snippet">
        <span className="w-full shrink-0">{thinkingSnippet(text)}</span>
      </div>
    );
  }
  const elapsed = Math.max(0, endedAtMs - startedAtMs);
  return (
    <Collapsible data-slot="thinking-settled">
      <CollapsibleTrigger className="text-muted-foreground group hover:text-foreground focus-visible:ring-ring/50 rounded-control flex min-h-row-sm w-full items-center gap-2 text-start text-sm outline-none focus-visible:ring-1">
        <Clock aria-hidden className="size-icon-md shrink-0" />
        <span>{elapsed < 1000 ? "Thought" : `Thought for ${formatDuration(elapsed)}`}</span>
        <ChevronRight aria-hidden className="size-icon-xs shrink-0 opacity-0 transition-[rotate,opacity] group-hover:opacity-100 group-focus-visible:opacity-100 group-data-[state=open]:rotate-90 group-data-[state=open]:opacity-100 motion-reduce:transition-none" />
      </CollapsibleTrigger>
      <CollapsibleContent className="text-muted-foreground max-h-action-list overflow-y-auto ps-6 pt-1 pb-2 text-sm whitespace-pre-wrap wrap-anywhere">
        {text}
      </CollapsibleContent>
    </Collapsible>
  );
}
