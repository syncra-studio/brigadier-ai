import type { OverrideRule, OverrideTarget, TaskCategory } from "@/ipc/generated";

/** The kinds of work the orchestrator hands to workers (not a chat or the orchestrator itself). */
export const WORKER_CATEGORIES: readonly TaskCategory[] = [
  "scout",
  "research",
  "implement",
  "review",
  "merge",
  "verify",
  "operate",
  "design",
];

/**
 * A "Don't give work" rule: `never` for every worker kind of work, anywhere, everywhere. Its
 * model gets no tasks but can still run a chat or session (the core's
 * `availability::worker_rule`).
 */
export function isNoWorkRule(rule: OverrideRule): boolean {
  return (
    rule.effect === "never" &&
    rule.categories.length === WORKER_CATEGORIES.length &&
    WORKER_CATEGORIES.every((category) => rule.categories.includes(category)) &&
    rule.areas.length === 0 &&
    rule.projectId === null
  );
}

/** A new "Don't give work" rule about `target` (what Rules' form adds). */
export function noWorkRule(target: OverrideTarget, id: string, createdAtMs: number): OverrideRule {
  return {
    id,
    effect: "never",
    target,
    categories: [...WORKER_CATEGORIES],
    areas: [],
    projectId: null,
    createdAtMs,
  };
}
