import type { BrainUse } from "@/ipc/generated";

/** Calls past which a worker that never asked the Brain nor taught it shows "Low Brain use". */
export const LOW_BRAIN_USE_CALLS = 50;

/**
 * A worker's Brain use on its row: "Brain 3↓ 2↑", its queries and the facts it wrote back.
 * `null` before it used the Brain at all (older tasks have no counters).
 */
export function brainUseLabel(use: BrainUse | undefined): string | null {
  if (!use || use.queries + use.factsIn + use.factsOut === 0) return null;
  return `Brain ${use.queries}↓ ${use.factsOut}↑`;
}

function plural(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

/** The counters in words, for a tooltip and the Inspector. */
export function brainUseText(use: BrainUse): string {
  return [
    `asked the Brain ${plural(use.queries, "time", "times")}`,
    `its brief carried ${plural(use.factsIn, "fact", "facts")}`,
    `taught it ${plural(use.factsOut, "fact", "facts")}`,
    plural(use.calls, "call", "calls"),
  ].join(" · ");
}

/** A long task with no Brain query and nothing written back: a gap to fix. */
export function lowBrainUse(use: BrainUse | undefined): boolean {
  return !!use && use.calls > LOW_BRAIN_USE_CALLS && use.queries === 0 && use.factsOut === 0;
}
