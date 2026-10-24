import { type FC, useLayoutEffect, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";

import { type PeerName, peerNameOf } from "@/app/conversation/activity/peerWords";
import { useViewConversation } from "@/app/conversation/viewContext";
import { MarkdownBlock, useKeptState } from "@/components/conversation";
import { cn } from "@/lib/utils";
import { useApp } from "@/state/store";

/** What the sender's row and the receiver's block of a message between sessions share. */

/** Another session as the open one names it; null once it is gone. */
export function usePeerName(id: string | undefined): PeerName | null {
  const here = useViewConversation()?.projectId ?? null;
  return useApp(
    useShallow((s) => {
      const name = peerNameOf(s.conversations, s.projects, id, here);
      return name ? { title: name.title, project: name.project } : null;
    }),
  );
}

/**
 * Markdown cut to six lines until "Show more": a message between sessions, in the receiver's
 * block and under the sender's row.
 */
export const FoldedMarkdown: FC<{ text: string; className?: string }> = ({ text, className }) => {
  const ref = useRef<HTMLDivElement>(null);
  const [expanded, setExpanded] = useKeptState(`more:${text.length}`, false);
  const [clipped, setClipped] = useState(false);
  useLayoutEffect(() => {
    const element = ref.current;
    if (!element) return;
    const measure = () => setClipped(element.scrollHeight > element.clientHeight + 1);
    measure();
    // The box stops growing at six lines; its content (markdown that loads late) goes on.
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    if (element.firstElementChild) observer.observe(element.firstElementChild);
    return () => observer.disconnect();
  }, []);
  return (
    <div className="flex min-w-0 flex-col">
      <div
        ref={ref}
        data-slot="peer-text"
        className={cn("min-w-0 wrap-break-word", !expanded && "max-h-peer-message overflow-hidden", clipped && !expanded && "plan-clip-fade", className)}
      >
        <div>
          <MarkdownBlock text={text} />
        </div>
      </div>
      {(clipped || expanded) && (
        <button
          type="button"
          aria-expanded={expanded}
          onClick={() => setExpanded(!expanded)}
          className="text-muted-foreground hover:text-foreground self-start pt-1 text-xs"
        >
          {expanded ? "Show less" : "Show more"}
        </button>
      )}
    </div>
  );
};
