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
 */
export function BarItem({ show, children }: { show: boolean; children: ReactNode }) {
  const [seen, setSeen] = useState(show);
  const [mounted, setMounted] = useState(show);
  const [open, setOpen] = useState(show);
  if (seen !== show) {
    setSeen(show);
    const instant = prefersReducedMotion();
    if (show) {
      setMounted(true);
      if (instant) setOpen(true);
    } else {
      setOpen(false);
      if (instant) setMounted(false);
    }
  }
  useEffect(() => {
    if (show) {
      // Laid out closed first, so the opening has somewhere to start from.
      let frame = requestAnimationFrame(() => {
        frame = requestAnimationFrame(() => setOpen(true));
      });
      return () => cancelAnimationFrame(frame);
    }
    const timer = window.setTimeout(() => setMounted(false), ITEM_MS);
    return () => window.clearTimeout(timer);
  }, [show]);
  if (!mounted) return null;
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
