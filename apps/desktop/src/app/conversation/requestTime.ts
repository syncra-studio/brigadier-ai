import { useEffect, useState } from "react";

import type { BlockState } from "@/app/conversation/blocks";
import type { Question } from "@/ipc/generated";
import { formatDuration } from "@/lib/format";

/** One clock for every live time on screen, so two of the same work read the same (D94). */
const ticks = new Set<(now: number) => void>();
let latest = Date.now();
let timer = 0;
function onTick(tick: (now: number) => void): () => void {
  if (ticks.size === 0) {
    latest = Date.now();
    timer = window.setInterval(() => {
      latest = Date.now();
      for (const each of ticks) each(latest);
    }, 1000);
  }
  ticks.add(tick);
  tick(latest);
  return () => {
    ticks.delete(tick);
    if (ticks.size === 0) window.clearInterval(timer);
  };
}

/** Now, ticking each second while `live`, on the clock all live times share. */
export function useTicking(live: boolean): number {
  const [now, setNow] = useState(() => (ticks.size > 0 ? latest : Date.now()));
  useEffect(() => (live ? onTick(setNow) : undefined), [live]);
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

/** How long a block's question cards waited for the user's answers: in all, and the open one so far (0 when none is open). */
export type CardWait = { total: number; open: number };

export function cardWait(questions: readonly Pick<Question, "createdAtMs" | "answer" | "answeredAtMs">[], now: number): CardWait {
  let total = 0;
  let open = 0;
  for (const question of questions) {
    // Answered with no time kept (older records): its wait is unknown, left out.
    const until = question.answeredAtMs ?? (question.answer === null ? now : null);
    if (until === null) continue;
    total += Math.max(0, until - question.createdAtMs);
    if (question.answer === null && question.answeredAtMs === null) open = Math.max(open, now - question.createdAtMs);
  }
  return { total, open };
}

/**
 * A turn's header: how long it worked, and how it ended. What a live turn waits on is its live
 * line's to say, once (`useThreadStatusHead`); the header only keeps the time.
 */
export function headerLabel(state: BlockState, elapsed: number, quota = false, interrupted = false, waited: CardWait | null = null): string {
  const time = formatDuration(elapsed);
  switch (state) {
    case "working": {
      if (quota) return `Waiting for quota · ${time}`;
      return elapsed < 1000 ? "Working" : `Working for ${time}`;
    }
    case "waiting":
      if (quota) return `Waiting for quota · ${time}`;
      // Its own question card waits for the user's answer (D161).
      if (waited?.open) return `Waiting for ${formatDuration(waited.open)}`;
      return `Waiting for you · ${time}`;
    case "done":
      // The time it waited on its question cards isn't work (D155, D161).
      if (waited && waited.total >= 1000) return `Worked for ${formatDuration(Math.max(0, elapsed - waited.total))} · waited for ${formatDuration(waited.total)}`;
      return `Worked for ${time}`;
    case "stopped":
      return `You stopped after ${time}`;
    // Said calmly: the error row under the turn tells what happened.
    case "failed":
      return interrupted ? `Interrupted after ${time}` : `Stopped after ${time}`;
  }
}

