import { useLayoutEffect, useRef, useState } from "react";

import { WorkerMarkdown } from "@/app/side-panel/WorkerMarkdown";
import { cn } from "@/lib/utils";

/** The brief, as the thread's first bubble: three lines, then "Show brief" for the rest. */
export function WorkerBrief({ text }: { text: string }) {
  const [open, setOpen] = useState(false);
  const [long, setLong] = useState(false);
  const body = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const element = body.current;
    if (!element || open) return;
    const measure = () => setLong(text.length > 0 && element.scrollHeight > element.clientHeight + 1);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [open, text]);
  return (
    <div data-slot="worker-brief" className="bg-secondary rounded-thread min-w-0 max-w-full flex flex-col gap-1 self-end px-3 py-2 text-sm">
      <div ref={body} data-slot="worker-instructions" className={cn("min-w-0 leading-relaxed wrap-anywhere", !open && "line-clamp-3")}>
        <WorkerMarkdown text={text} />
      </div>
      {(long || open) && (
        <button type="button" onClick={() => setOpen(!open)} className="text-foreground/60 hover:text-foreground self-start text-xs">
          {open ? "Show less" : "Show brief"}
        </button>
      )}
    </div>
  );
}
