/**
 * The collapsed sidebar's peek: resting the pointer on its trigger (the strip's mark, or the
 * window's start edge while the sidebar is hidden) floats the full panel over the content until
 * the pointer leaves both. This is the timing and its rules, apart from the DOM:
 * - it opens once the pointer has rested on the trigger for OPEN_DELAY_MS, if `canOpen` says so
 *   then (sidebar collapsed, no menu or dialog open, no resizing);
 * - it closes CLOSE_DELAY_MS after the pointer leaves the trigger and the panel, unless
 *   `holdOpen` (a menu or dialog is open), which waits for it;
 * - while closing it plays its exit for `exitMs()`; the pointer back on the trigger reopens it;
 * - dismissed (Escape, a click outside, navigating), it won't open again until the pointer
 *   has been off both the trigger and the panel.
 */

export const PEEK_OPEN_DELAY_MS = 100;
export const PEEK_CLOSE_DELAY_MS = 300;
export const PEEK_EXIT_MS = 200;

export type PeekPhase = "closed" | "open" | "closing";
/** What the pointer is over: the trigger, the floating panel, or anything else. */
export type PeekZone = "trigger" | "panel" | "outside";

export type PeekTimers = {
  set: (run: () => void, ms: number) => unknown;
  clear: (handle: unknown) => void;
};

const browserTimers: PeekTimers = {
  set: (run, ms) => setTimeout(run, ms),
  clear: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
};

export type PeekOptions = {
  onChange: (phase: PeekPhase) => void;
  canOpen: () => boolean;
  holdOpen: () => boolean;
  exitMs: () => number;
  timers?: PeekTimers;
};

export type Peek = {
  phase: () => PeekPhase;
  /** The pointer moved over `zone`. */
  point: (zone: PeekZone) => void;
  /** Close it now (with its exit), and keep it shut until the pointer is off the trigger. */
  dismiss: () => void;
  /** Close it at once, without its exit (the sidebar was opened for real). */
  reset: () => void;
  dispose: () => void;
};

export function createPeek({ onChange, canOpen, holdOpen, exitMs, timers = browserTimers }: PeekOptions): Peek {
  let phase: PeekPhase = "closed";
  let zone: PeekZone = "outside";
  let suppressed = false;
  let opening: unknown = null;
  let closing: unknown = null;
  let exiting: unknown = null;

  const stop = (handle: unknown) => {
    if (handle !== null) timers.clear(handle);
    return null;
  };
  const stopAll = () => {
    opening = stop(opening);
    closing = stop(closing);
    exiting = stop(exiting);
  };
  const go = (next: PeekPhase) => {
    if (phase === next) return;
    phase = next;
    onChange(next);
  };
  const close = () => {
    opening = stop(opening);
    closing = stop(closing);
    if (phase !== "open") return;
    const ms = exitMs();
    if (ms <= 0) {
      go("closed");
      return;
    }
    go("closing");
    exiting = timers.set(() => {
      exiting = null;
      go("closed");
    }, ms);
  };
  const armClose = () => {
    if (closing !== null) return;
    closing = timers.set(() => {
      closing = null;
      if (zone !== "outside" || phase !== "open") return;
      // A menu or dialog opened from the panel keeps it up; look again a moment later.
      if (holdOpen()) armClose();
      else close();
    }, PEEK_CLOSE_DELAY_MS);
  };

  return {
    phase: () => phase,
    point(next) {
      zone = next;
      if (next === "outside") suppressed = false;
      if (next !== "trigger") opening = stop(opening);
      if (next === "trigger" && suppressed) return;
      if (phase === "open") {
        if (next === "outside") armClose();
        else closing = stop(closing);
        return;
      }
      if (next !== "trigger") return;
      if (phase === "closing") {
        exiting = stop(exiting);
        if (canOpen()) go("open");
        return;
      }
      if (opening !== null) return;
      opening = timers.set(() => {
        opening = null;
        if (zone === "trigger" && !suppressed && canOpen()) go("open");
      }, PEEK_OPEN_DELAY_MS);
    },
    dismiss() {
      suppressed = true;
      opening = stop(opening);
      close();
    },
    reset() {
      stopAll();
      go("closed");
    },
    dispose: stopAll,
  };
}
