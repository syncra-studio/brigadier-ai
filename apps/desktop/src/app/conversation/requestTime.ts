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

/** What a turn over says while background commands it started still run, or one's end is owed its turn (D21). */
export function commandsWaitWords(commands: number): string {
  return commands === 1 ? "Waiting for a command" : `Waiting for ${commands} commands`;
}

/**
 * A turn's header: how long it worked, and how it ended. What a live turn waits on is its live
 * line's to say, once (`useThreadStatusHead`); the header only keeps the time.
 */
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

