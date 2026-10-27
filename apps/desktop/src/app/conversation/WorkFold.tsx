import { type ReactNode, useLayoutEffect, useRef, useState } from "react";

/** Opening by a click takes 300ms on a fast-out curve; closing, also when an answer folds the work, 150ms on a quicker one. */
export const OPEN = { duration: 300, easing: "cubic-bezier(0.19, 1, 0.22, 1)" } as const;
export const CLOSE = { duration: 150, easing: "cubic-bezier(0.23, 1, 0.32, 1)" } as const;
/** With less motion, both ways take this long, with no slide. */
export const REDUCED_MS = 150;

/**
 * Whether it opens and closes at once: while the window is hidden, where WebKit holds animations
 * still and a fold that closed there would stay open until it is shown.
 */
function atOnce(): boolean {
  return document.hidden;
}

function lessMotion(): boolean {
  return typeof window.matchMedia === "function" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/**
 * A run of a turn's work that folds away under its header. Its body is mounted when it opens and
 * unmounted when it has closed. Height (in px, from what it measures), opacity and an 8px slide
 * run on one clock, the Web Animations API's, and the body leaves in that animation's own finish,
 * so what follows moves with the height frame by frame and nothing jumps when it settles. Turned
 * around midway, it goes back from where it is. Only a click (`clicked`) animates opening; opened
 * by anything else (a reveal) it is open at once, while closing always takes its 150ms. With less
 * motion both take 150ms with no slide; hidden, it opens and closes at once. It never animates on
 * mounting.
 * While Find searches (`searched`), closed it keeps its body mounted, inert and, once still,
 * hidden (`data-kept`), so Find counts what it holds; going to a match there opens it.
 */
export function WorkFold({ open, clicked = false, searched = false, children }: {
  open: boolean;
  /** The change to `open` is the user's click on its header. */
  clicked?: boolean;
  searched?: boolean;
  children: ReactNode;
}) {
  const [mounted, setMounted] = useState(open || searched);
  const body = useRef<HTMLDivElement>(null);
  const running = useRef<Animation[]>([]);
  const growing = useRef<ResizeObserver | null>(null);
  const was = useRef(open);
  const searchedNow = useRef(searched);
  // Mounted the moment it opens or Find searches; with less motion, gone the moment it closes.
  if ((open || searched) && !mounted) setMounted(true);
  if (!open && !searched && mounted && atOnce()) setMounted(false);

  useLayoutEffect(() => {
    if (was.current === open) return;
    was.current = open;
    const element = body.current;
    const inner = element?.firstElementChild;
    if (!element || !(inner instanceof HTMLElement)) return;
    delete element.dataset.kept;
    // From the height it shows now: its full height closing, nothing opening, or wherever an
    // animation going the other way left it.
    const turning = running.current.length > 0;
    const from = turning ? element.getBoundingClientRect().height : open ? 0 : element.offsetHeight;
    // Its fade and slide too: turned around, they go back from where they are, with no flash.
    const shown = turning ? getComputedStyle(inner) : null;
    const at = shown ? { opacity: Number(shown.opacity), transform: shown.transform } : null;
    for (const animation of running.current) animation.cancel();
    running.current = [];
    growing.current?.disconnect();
    growing.current = null;
    // It clips only while it moves: open, focus rings and shadows inside it show in full.
    element.dataset.moving = "";
    // Opened by anything but a click, it is open at once.
    if (atOnce() || (open && !clicked)) {
      delete element.dataset.moving;
      if (!open) element.dataset.kept = "";
      return;
    }
    let to = open ? inner.offsetHeight : 0;
    const reduced = lessMotion();
    const slide = reduced ? "none" : `translateY(calc(${getComputedStyle(element).getPropertyValue("--spacing-fold-slide")} * -1))`;
    const timing = {
      ...(open ? OPEN : CLOSE),
      ...(reduced && { duration: REDUCED_MS }),
      fill: open ? ("none" as const) : ("forwards" as const),
    };
    const height = element.animate([{ height: `${from}px` }, { height: `${to}px` }], timing);
    const fade = inner.animate(
      open
        ? [at ?? { opacity: 0, transform: slide }, { opacity: 1, transform: "none" }]
        : [at ?? { opacity: 1, transform: "none" }, { opacity: 0, transform: slide }],
      timing,
    );
    running.current = [height, fade];
    const finish = () => {
      const ended = running.current;
      running.current = [];
      growing.current?.disconnect();
      growing.current = null;
      if (open) delete element.dataset.moving;
      // Closed, it stays at 0 (the fill) until it is gone; open, it is back to its own height,
      // which is the height it ended on. Closed while Find searches, it stays, hidden by its style.
      if (!open && searchedNow.current) {
        element.dataset.kept = "";
        for (const animation of ended) animation.cancel();
        delete element.dataset.moving;
      } else if (!open) setMounted(false);
    };
    height.onfinish = finish;
    // What grows while it opens (a card whose view loads just then) moves the height it opens
    // to: from where it is, over the time left, so it never ends on a jump.
    if (open && typeof ResizeObserver !== "undefined") {
      const started = performance.now();
      growing.current = new ResizeObserver(() => {
        const target = inner.offsetHeight;
        const current = running.current[0];
        if (target === to || !current) return;
        const now = element.getBoundingClientRect().height;
        current.onfinish = null;
        current.cancel();
        to = target;
        const left = Math.max(0, timing.duration - (performance.now() - started));
        const next = element.animate([{ height: `${now}px` }, { height: `${to}px` }], { ...timing, duration: left });
        next.onfinish = finish;
        running.current = [next, fade];
      });
      growing.current.observe(inner);
    }
    // Only `open` changing starts a move; `clicked` is read with it.
  }, [open, clicked]);

  // After the one above, which starts its close: Find is done, and it is closed and still, so its body goes.
  useLayoutEffect(() => {
    searchedNow.current = searched;
    if (!open && !searched && running.current.length === 0) setMounted(false);
  }, [open, searched]);

  // Mounted closed for Find, or closed at once: hidden from the first frame. A close that
  // animates is hidden once it is over (its finish).
  useLayoutEffect(() => {
    if (mounted && !open && running.current.length === 0) body.current?.setAttribute("data-kept", "");
  }, [mounted, open]);

  // Gone midway: it stops watching what it held.
  useLayoutEffect(() => () => growing.current?.disconnect(), []);

  if (!mounted) return null;
  return (
    <div
      ref={body}
      data-slot="request-fold"
      data-fold={open ? "open" : "closed"}
      inert={!open}
      // Closed and still, kept for Find: no height, nothing shown, its text still laid out to find.
      className="data-moving:overflow-hidden data-kept:invisible data-kept:h-0 data-kept:overflow-hidden"
    >
      {/* min-w-0: a long unbroken line (a branch in code) wraps instead of widening the fold. */}
      <div className="min-w-0">{children}</div>
    </div>
  );
}
