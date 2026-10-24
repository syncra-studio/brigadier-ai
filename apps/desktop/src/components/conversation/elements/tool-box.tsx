import { type ReactNode, useEffect, useRef, useState } from "react";

import { cn } from "@/lib/utils";
import { useKeptState } from "@/components/conversation/row-memory";

/**
 * A quiet box of exact text: a tool's input or output, a command a card asks about, a brief or
 * a message the lead sent. Mono unless `prose`; an optional small title over it. It scrolls
 * past 240px; `clamp` shows three lines until Expand.
 */
export function ToolBox({
  title,
  prose = false,
  clamp = false,
  className,
  children,
}: {
  title?: string;
  prose?: boolean;
  clamp?: boolean;
  className?: string;
  children: ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [expanded, setExpanded] = useKeptState(`box:${title ?? ""}`, false);
  const [clamped, setClamped] = useState(false);
  useEffect(() => {
    const element = ref.current;
    if (!clamp || !element) return;
    const observer = new ResizeObserver(() => setClamped(element.scrollHeight > element.clientHeight + 1));
    observer.observe(element);
    return () => observer.disconnect();
  }, [clamp]);
  return (
    <div data-slot="tool-card" className={cn("bg-code-surface text-foreground/80 rounded-control flex flex-col", className)}>
      {title && <span className="text-muted-foreground px-3 pt-2 text-xs">{title}</span>}
      <div className={cn("max-h-60 overflow-auto px-3 pb-2", title ? "pt-1" : "pt-2")}>
        <div
          ref={ref}
          data-selectable
          className={cn(
            "break-words whitespace-pre-wrap",
            prose ? "text-sm" : "text-code font-mono",
            clamp && !expanded && "line-clamp-3",
          )}
        >
          {children}
        </div>
      </div>
      {clamp && (clamped || expanded) && (
        <div className="flex shrink-0 justify-end p-1">
          <button
            type="button"
            onClick={() => setExpanded(!expanded)}
            className="text-foreground/50 hover:bg-foreground/8 text-code rounded-lg px-2 py-0.5 leading-4.5 transition-colors"
          >
            {expanded ? "Collapse" : "Expand"}
          </button>
        </div>
      )}
    </div>
  );
}
