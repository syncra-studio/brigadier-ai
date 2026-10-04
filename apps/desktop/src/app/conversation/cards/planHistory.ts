import type { Plan } from "@/ipc/generated";

/**
 * What a session's Plan section lists under "Earlier plans", newest first: each plan before the
 * current one (`older`, oldest first).
 */
export function earlierPlans(
  plans: Readonly<Record<string, Plan>>,
  plan: Plan,
  older: readonly string[],
): Plan[] {
  const list: Plan[] = [];
  const seen = new Set([plan.id]);
  for (const id of older.toReversed()) {
    const before = plans[id];
    if (!before || seen.has(id)) continue;
    seen.add(id);
    list.push(before);
  }
  return list;
}
