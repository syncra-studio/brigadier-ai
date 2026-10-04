import type { Plan } from "@/ipc/generated";

/**
 * What a session's Plan section lists under "Earlier plans", newest first: the current plan's
 * earlier revisions, then each plan before it (`older`, oldest first) with the revisions it
 * replaced. A revision's newer plan is superseded and kept from the session's own plans, so
 * this is the only place its history shows.
 */
export function earlierPlans(
  plans: Readonly<Record<string, Plan>>,
  plan: Plan,
  older: readonly string[],
): Plan[] {
  const list: Plan[] = [];
  const seen = new Set([plan.id]);
  const walk = (from: string | null) => {
    for (let id = from; id && !seen.has(id); ) {
      seen.add(id);
      const before = plans[id];
      if (!before) break;
      list.push(before);
      id = before.revises;
    }
  };
  walk(plan.revises);
  for (const olderId of older.toReversed()) walk(olderId);
  return list;
}
