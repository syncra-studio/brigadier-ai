import { useEffect, useState } from "react";

import type { BlockState } from "@/app/conversation/blocks";
import type { PhaseView } from "@/app/conversation/phaseView";
import { formatDuration } from "@/lib/format";

/** Now, ticking each second while `live`. */
export function useTicking(live: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!live) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [live]);
  return now;
}

/** Seconds since `from`, ticking while `live`. */
export function useElapsed(from: number, to: number | null, live: boolean): number {
  const now = useTicking(live);
  return Math.max(0, (live || to === null ? now : to) - from);
}

/** A turn's header: how long it worked, and how it ended. */
export function headerLabel(state: BlockState, elapsed: number, quota = false, interrupted = false): string {
  const time = formatDuration(elapsed);
  switch (state) {
    case "working": {
      if (quota) return `Waiting for quota · ${time}`;
      return elapsed < 1000 ? "Working" : `Working for ${time}`;
    }
    case "waiting":
      if (quota) return `Waiting for quota · ${time}`;
      return `Waiting for you · ${time}`;
    case "done":
      return `Worked for ${time}`;
    case "stopped":
      return `You stopped after ${time}`;
    // Said calmly: the error row under the turn tells what happened.
    case "failed":
      return interrupted ? `Interrupted after ${time}` : `Stopped after ${time}`;
  }
}

/**
 * An overnight run's header: "Overnight · Windows support — Phase 2 of 3 · Fix · 1h 4m" while it
 * works, "— ◐ 1 of 3 phases done · 6h 12m" once over; ticking only while it works.
 */
export function phaseLabel(phase: PhaseView, elapsed: number): string {
  const state = phase.mark ? `${phase.mark} ${phase.word}` : phase.word;
  return phase.startedAtMs === null ? `${phase.title} — ${state}` : `${phase.title} — ${state} · ${formatDuration(elapsed)}`;
}
