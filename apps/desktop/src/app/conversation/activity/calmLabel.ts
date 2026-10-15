import { useEffect, useState } from "react";

/** A live label changes at most this often: fast steps never flicker through their names. */
export const LABEL_EVERY_MS = 1000;

/**
 * The pace of a live label: a new label shows at once when the shown one has stood for
 * `LABEL_EVERY_MS`, else once it has, and then the newest label wins. The clock is passed in, so
 * it is the same with a test's clock.
 */
export class LabelPace {
  private shown: string | null = null;
  private since = Number.NEGATIVE_INFINITY;
  private readonly everyMs: number;

  constructor(everyMs = LABEL_EVERY_MS) {
    this.everyMs = everyMs;
  }

  /** Shows `label` at `now` whatever the pace. */
  show(label: string, now: number): { shown: string; wakeAt: null } {
    if (this.shown !== label) {
      this.shown = label;
      this.since = now;
    }
    return { shown: label, wakeAt: null };
  }

  /** What to show at `now` for the newest `label`, and when to ask again (`null`: no need). */
  next(label: string, now: number): { shown: string; wakeAt: number | null } {
    if (this.shown === label) return { shown: label, wakeAt: null };
    if (this.shown === null || now - this.since >= this.everyMs) {
      this.shown = label;
      this.since = now;
      return { shown: label, wakeAt: null };
    }
    return { shown: this.shown, wakeAt: this.since + this.everyMs };
  }
}

/**
 * A live label, paced by `LabelPace`; when `calm` is off (a group sums up, the turn ended) the
 * label shows as it is at once. `pending`: a newer label waits for its turn.
 */
export function useCalmLabel(label: string, calm: boolean): { shown: string; pending: boolean } {
  const [pace] = useState(() => new LabelPace());
  const [shown, setShown] = useState(() => pace.next(label, Date.now()).shown);
  useEffect(() => {
    const step = () => {
      // Not calm, it shows at once, and counts as shown from now: a calm label after it waits its turn.
      const { shown: next, wakeAt } = calm ? pace.next(label, Date.now()) : pace.show(label, Date.now());
      setShown(next);
      return wakeAt;
    };
    const wakeAt = step();
    if (wakeAt === null) return;
    const timer = window.setTimeout(step, Math.max(0, wakeAt - Date.now()));
    return () => window.clearTimeout(timer);
  }, [pace, label, calm]);
  if (!calm) return { shown: label, pending: false };
  return { shown, pending: shown !== label };
}
