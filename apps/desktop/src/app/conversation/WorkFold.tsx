import { type ReactNode, useLayoutEffect, useRef, useState } from "react";

/** Opening takes 300ms on a fast-out curve; closing 160ms (§4.2 of THREAD-PARITY-PLAN.md). */
const OPEN = { duration: 300, easing: "cubic-bezier(0.19, 1, 0.22, 1)" } as const;
const CLOSE = { duration: 160, easing: "cubic-bezier(0.4, 0, 0.2, 1)" } as const;

function reducedMotion(): boolean {
  return typeof window.matchMedia === "function" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/**
 * A run of a turn's work that folds away under its header. Its body is mounted when it opens and
 * unmounted when it has closed. Height (in px, from what it measures), opacity and an 8px slide
 * run on one clock, the Web Animations API's, and the body leaves in that animation's own finish,
 * so what follows moves with the height frame by frame and nothing jumps when it settles. Turned
 * around midway, it goes back from where it is. With less motion it opens and closes at once.
 */
export function WorkFold({ open, children }: { open: boolean; children: ReactNode }) {
  const [mounted, setMounted] = useState(open);
  const body = useRef<HTMLDivElement>(null);
  const running = useRef<Animation[]>([]);
  const was = useRef(open);
  // Mounted the moment it opens; with less motion, gone the moment it closes.
  if (open && !mounted) setMounted(true);
  if (!open && mounted && reducedMotion()) setMounted(false);

  useLayoutEffect(() => {
    if (was.current === open) return;
    was.current = open;
    const element = body.current;
    const inner = element?.firstElementChild;
    if (!element || !(inner instanceof HTMLElement)) return;
    // From the height it shows now: its full height closing, nothing opening, or wherever an
    // animation going the other way left it.
    const from = running.current.length > 0 ? element.getBoundingClientRect().height : open ? 0 : element.offsetHeight;
    for (const animation of running.current) animation.cancel();
    running.current = [];
    // It clips only while it moves: open, focus rings and shadows inside it show in full.
    element.dataset.moving = "";
    if (reducedMotion()) {
      delete element.dataset.moving;
      return;
    }
    const to = open ? inner.offsetHeight : 0;
    const slide = `translateY(calc(${getComputedStyle(element).getPropertyValue("--spacing-fold-slide")} * -1))`;
    const timing = { ...(open ? OPEN : CLOSE), fill: open ? ("none" as const) : ("forwards" as const) };
    const height = element.animate([{ height: `${from}px` }, { height: `${to}px` }], timing);
    const fade = inner.animate(
      open
        ? [{ opacity: 0, transform: slide }, { opacity: 1, transform: "none" }]
        : [{ opacity: 1, transform: "none" }, { opacity: 0, transform: slide }],
      timing,
    );
    running.current = [height, fade];
    height.onfinish = () => {
      running.current = [];
      if (open) delete element.dataset.moving;
      // Closed, it stays at 0 (the fill) until it is gone; open, it is back to its own height,
      // which is the height it ended on.
      if (!open) setMounted(false);
    };
  }, [open]);

  if (!mounted) return null;
  return (
    <div ref={body} data-slot="request-fold" data-fold={open ? "open" : "closed"} className="data-moving:overflow-hidden">
      {/* min-w-0: a long unbroken line (a branch in code) wraps instead of widening the fold. */}
      <div className="min-w-0">{children}</div>
    </div>
  );
}
