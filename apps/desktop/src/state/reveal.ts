import { create } from "zustand";

import { jumpInThread, threadActions } from "@/components/assistant-ui/thread-scroll";

/**
 * A message between sessions the thread is asked to show (SESSION-MESSAGING-PLAN.md §6.3): its
 * page loaded, the thread unfolds what holds it, scrolls it to the centre and flashes it once.
 * Each jump has its own `nonce`, so asking for the same message again lands again.
 */
export type Reveal = { conversationId: string; messageId: string; nonce: number };

export const useReveal = create<{ reveal: Reveal | null }>(() => ({ reveal: null }));

let nonce = 0;

/** Asks the thread of `conversationId` to show `messageId`, once what holds it is loaded. */
export function startReveal(conversationId: string, messageId: string): void {
  nonce += 1;
  useReveal.setState({ reveal: { conversationId, messageId, nonce } });
}

/** The jump to `nonce` is over (landed, or given up). */
export function endReveal(done: number): void {
  if (useReveal.getState().reveal?.nonce === done) useReveal.setState({ reveal: null });
}

/** The message `conversationId`'s thread is asked to show now, if any. */
export function useRevealing(conversationId: string | null): string | null {
  return useReveal((s) => (s.reveal && s.reveal.conversationId === conversationId ? s.reveal.messageId : null));
}

/** The row or block of `messageId` in a shown thread. */
function shownElement(messageId: string): HTMLElement | null {
  // A row of several messages names each of them.
  const found = document.querySelectorAll<HTMLElement>(`[data-peer-message~="${CSS.escape(messageId)}"]`);
  return [...found].find((element) => element.closest("[data-slot=aui_thread-viewport]") && element.getClientRects().length > 0) ?? null;
}

/** Whether a fold around `element` still opens: an animation runs on something that holds it. */
function settling(element: Element): boolean {
  return document.getAnimations().some((animation) => {
    // A scroll-driven one (a long group's fading edges) runs as long as its list scrolls: no fold.
    if (animation.playState !== "running" || !(animation.timeline instanceof DocumentTimeline)) return false;
    const target = animation.effect instanceof KeyframeEffect ? animation.effect.target : null;
    return target instanceof Element && target !== element && target.contains(element);
  });
}

/** How long a landing waits for its row to show and its folds to settle, and keeps it centred after. */
const LAND_WITHIN_MS = 4000;
/** Frames the row must keep its place before the thread scrolls to it. */
const STILL_FRAMES = 3;
/** The flash: a tint held this long, then fading. */
const FLASH_MS = 200;
const FADE_MS = 400;

/**
 * Lands the jump `nonce` on `messageId` once its row is in the thread and the folds around it
 * have finished opening: the thread's viewport scrolls it to the centre (the thread's scroll
 * controller takes that as the reader's own scroll: it stops following), then it flashes once
 * and is kept centred while what's around it settles (`keepCentred`). Returns a call that
 * cancels the landing.
 */
export function landReveal(messageId: string, done: number): () => void {
  cancelAnimationFrame(keeping);
  const until = performance.now() + LAND_WITHIN_MS;
  let frame = 0;
  let still = 0;
  let last: number | null = null;
  const step = () => {
    const element = shownElement(messageId);
    const now = performance.now();
    if (element) {
      const top = element.getBoundingClientRect().top;
      still = !settling(element) && top === last ? still + 1 : 0;
      last = top;
      if (still >= STILL_FRAMES || now > until) {
        land(element);
        endReveal(done);
        return;
      }
    } else if (now > until) {
      endReveal(done);
      return;
    }
    frame = requestAnimationFrame(step);
  };
  frame = requestAnimationFrame(step);
  return () => cancelAnimationFrame(frame);
}

function land(element: HTMLElement): void {
  const viewport = element.closest<HTMLElement>("[data-slot=aui_thread-viewport]");
  jumpInThread(viewport);
  // A list that scrolls within the thread (a long group's rows) first brings the row into its own
  // view, then the thread centres it.
  for (let holder = element.parentElement; holder && holder !== viewport; holder = holder.parentElement) {
    if (holder.scrollHeight > holder.clientHeight && /auto|scroll/.test(getComputedStyle(holder).overflowY)) centre(holder, element);
  }
  if (viewport) {
    centre(viewport, element);
    keepCentred(viewport, element);
  }
  flash(element);
}

/** The frame request of the landed jump keeping its message centred (`keepCentred`). */
let keeping = 0;

/**
 * Content around a landed message can still change its height: a reply waiting on its renderer
 * (loaded with the first reply, late on a slow machine) gives way to the reply. While the jump is
 * the user's last action in the thread, each time the thread settles again the message is
 * centred again, for as long as a landing waits.
 */
function keepCentred(viewport: HTMLElement, element: HTMLElement): void {
  const until = performance.now() + LAND_WITHIN_MS;
  let since = threadActions();
  let still = 0;
  let last: number | null = null;
  const step = () => {
    if (!element.isConnected || threadActions() !== since || performance.now() > until) return;
    const top = element.getBoundingClientRect().top;
    const loading = viewport.querySelector("[data-slot=reply-loading]") !== null;
    still = !loading && !settling(element) && top === last ? still + 1 : 0;
    last = top;
    if (still >= STILL_FRAMES && Math.abs(centred(viewport, element) - viewport.scrollTop) >= 1) {
      jumpInThread(viewport);
      since = threadActions();
      centre(viewport, element);
      still = 0;
    }
    keeping = requestAnimationFrame(step);
  };
  keeping = requestAnimationFrame(step);
}

/** The scrollTop of `scroller` that puts `element` at its centre, as far as it can. */
function centred(scroller: HTMLElement, element: HTMLElement): number {
  const box = scroller.getBoundingClientRect();
  const rect = element.getBoundingClientRect();
  const off = rect.top + rect.height / 2 - (box.top + box.height / 2);
  return Math.max(0, Math.min(scroller.scrollHeight - scroller.clientHeight, scroller.scrollTop + off));
}

/** Scrolls `scroller` so `element` sits at its centre, as far as it can. */
function centre(scroller: HTMLElement, element: HTMLElement): void {
  // oxlint-disable-next-line brigadier/thread-scroll-helper -- `land` said it jumps before it centres.
  scroller.scrollTop = centred(scroller, element);
}

/** The flash's tint: the link colour, faint (what a picked sidebar row shows). */
const TINT = "bg-link/20";

/** One tint over the row (or the part of it marked `data-flash`), held a moment, then gone. */
function flash(element: HTMLElement): void {
  const target = element.querySelector<HTMLElement>("[data-flash]") ?? element;
  // The tint as the design tokens make it, read off the element itself.
  target.classList.add(TINT);
  const tint = getComputedStyle(target).backgroundColor;
  target.classList.remove(TINT);
  const total = FLASH_MS + FADE_MS;
  element.dataset["revealed"] = "true";
  target.animate(
    [
      { backgroundColor: tint, offset: 0 },
      { backgroundColor: tint, offset: FLASH_MS / total },
      { backgroundColor: "transparent", offset: 1 },
    ],
    { duration: total, easing: "ease-out" },
  );
}
