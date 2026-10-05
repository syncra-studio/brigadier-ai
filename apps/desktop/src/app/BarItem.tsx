import { useEffect, useState, type ReactNode } from "react";

import { cn } from "@/lib/utils";

const ITEM_MS = 200;

function prefersReducedMotion(): boolean {
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/**
 * Something on the bar that is only sometimes there: it grows from nothing and fades in, and
 * shrinks and fades out, over 200ms, so its neighbours glide. Leaving, it can't be focused or
 * clicked at once; it is gone when the motion ends (at once with reduced motion). Shown again
 * while leaving, it turns back.
 *
 * Whether it is there follows `show` itself, not a copy of it set while rendering: with the
 * window hidden, such a copy was lost and the item never came back. The timers only open it
 * and keep it while it leaves, so a late one can't hide it while it should show.
 */
export function BarItem({ show, children }: { show: boolean; children: ReactNode }) {
  // Kept while leaving, until the motion ends.
  const [kept, setKept] = useState(show);
  // Has had the closed frame it opens from.
  const [opened, setOpened] = useState(show);
  useEffect(() => {
    if (show) {
      const open = () => {
        setKept(true);
        setOpened(true);
      };
      // Laid out closed first, so the opening has somewhere to start from. Frames stop while
      // the window is hidden, so a timer opens it too.
      let frame = requestAnimationFrame(() => {
        frame = requestAnimationFrame(open);
      });
      const timer = window.setTimeout(open, ITEM_MS / 4);
      return () => {
        cancelAnimationFrame(frame);
        window.clearTimeout(timer);
      };
    }
    const timer = window.setTimeout(() => {
      setKept(false);
      setOpened(false);
    }, ITEM_MS);
    return () => window.clearTimeout(timer);
  }, [show]);
  const instant = prefersReducedMotion();
  if (!show && (!kept || instant)) return null;
  const open = show && (opened || instant);
  return (
    <div
      inert={!show}
      aria-hidden={!show || undefined}
      className={cn(
        "ease-standard grid shrink-0 transition-[grid-template-columns,opacity] duration-200 motion-reduce:transition-none",
        open ? "grid-cols-[1fr] opacity-100" : "grid-cols-[0fr] opacity-0",
      )}
    >
      <div className="flex min-w-0 items-center overflow-hidden">{children}</div>
    </div>
  );
}
