import { useEffect, useState } from "react";

import type { BlockState } from "@/app/conversation/blocks";
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

/** What a turn over says while the workers it started still work. */
export function workersWaitWords(workers: number): string {
  return workers === 1 ? "Waiting for a worker" : `Waiting for ${workers} workers`;
}

/**
 * A turn's header: how long it worked, and how it ended. `workers`: a turn over whose workers
 * still work says it waits for them, and for how long.
 */
export function headerLabel(state: BlockState, elapsed: number, quota = false, interrupted = false, workers = 0): string {
  const time = formatDuration(elapsed);
  if (workers > 0) return `${workersWaitWords(workers)} · ${time}`;
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

