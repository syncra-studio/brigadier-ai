import type { MergedModel } from "@/ipc/generated";
import { CATEGORIES, joinWords, PLAIN_KIND_WORDS } from "@/lib/routing";

/** A kind of work counts among a model's best within this much of its top score. */
const BEST_WITHIN = 0.5;
/** "Best at" names at most this many kinds of work. */
const BEST_SHOWN = 3;

/**
 * "Best for writing code, reviewing changes and resolving conflicts": the kinds of work a
 * model scores highest for, in plain words. `null` for a model without scores.
 */
export function bestFor(model: MergedModel): string | null {
  const scored = CATEGORIES.flatMap((category) => {
    const strength = model.strengths[category];
    return strength === null || strength === undefined ? [] : [{ category, strength }];
  }).toSorted((a, b) => b.strength - a.strength);
  const top = scored[0]?.strength;
  if (top === undefined) return null;
  const best = scored.filter((entry) => entry.strength >= top - BEST_WITHIN).slice(0, BEST_SHOWN);
  return `Best for ${joinWords(best.map((entry) => PLAIN_KIND_WORDS[entry.category]))}`;
}
