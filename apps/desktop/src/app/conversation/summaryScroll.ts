import { type UIEvent, useLayoutEffect, useRef } from "react";

import { keptScroll } from "@/app/conversation/summaryState";

/** How long a reopened summary card keeps restoring its offset while its rows arrive. */
const RESTORE_MS = 1000;

/**
 * An element of the summary that reopens where it was scrolled to, kept under `key`. While it is
 * `active`, every scroll counts, a clamp too (so a card that stops overflowing forgets its
 * offset). Just after it shows it is restoring instead: rows that read asynchronously (the
 * branch's diff, a pull request) may not be there yet, so the offset is applied again as they
 * arrive, until it is reached, the element is scrolled by hand, or a moment has passed.
 */
export function useKeptScroll(key: string, active: boolean) {
  const ref = useRef<HTMLDivElement>(null);
  const restoring = useRef(false);
  const onScroll = (event: UIEvent<HTMLElement>) => {
    const { scrollTop } = event.currentTarget;
    if (!active) return;
    if (restoring.current) {
      if (scrollTop !== keptScroll.get(key)) return;
      restoring.current = false;
    }
    if (scrollTop > 0) keptScroll.set(key, scrollTop);
    else keptScroll.delete(key);
  };
  useLayoutEffect(() => {
    const element = ref.current;
    const top = keptScroll.get(key) ?? 0;
    if (!active || !element || top === 0) return;
    restoring.current = true;
    const restore = () => {
      if (restoring.current && element.scrollTop !== top) element.scrollTop = top;
    };
    const stop = () => {
      restoring.current = false;
    };
    restore();
    const observer = new ResizeObserver(restore);
    observer.observe(element);
    if (element.firstElementChild) observer.observe(element.firstElementChild);
    const timer = setTimeout(stop, RESTORE_MS);
    for (const type of ["wheel", "pointerdown", "keydown"]) element.addEventListener(type, stop);
    return () => {
      stop();
      observer.disconnect();
      clearTimeout(timer);
      for (const type of ["wheel", "pointerdown", "keydown"]) element.removeEventListener(type, stop);
    };
  }, [active, key]);
  return { ref, onScroll };
}

/**
 * The summary's column of cards, pinned in the pane or floating from the top bar: inset from the
 * top bar and the pane's end, each card as tall as what it shows. Where the cards can't all fit at
 * their least height the column scrolls too, so every card stays reachable.
 */
export const STACK =
  "pointer-events-none flex max-h-full min-h-0 w-summary-stack max-w-full flex-col gap-summary-inset overflow-y-auto p-summary-inset";
