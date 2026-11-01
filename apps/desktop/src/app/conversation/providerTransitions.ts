import type { BlockOrchestratorStep, BlockSeam } from "@/app/conversation/blocks";
import type { ThreadEntry } from "@/components/conversation";
import type { ModelChoice, OrchestratorStep } from "@/ipc/generated";

export type ProviderTransition = Extract<OrchestratorStep["kind"], { type: "providerTransition" }>;

/** The stable persisted id survives stream replay and a fresh board read. */
export function transitionSteps<T extends BlockOrchestratorStep>(steps: readonly T[]): T[] {
  const ids = new Set<string>();
  const transitions = steps.filter((step) => step.kind.type === "providerTransition" && !step.kind.taskId);
  return steps.filter((step) => {
    if (step.kind.type === "providerTransition") {
      if (ids.has(step.kind.id)) return false;
      ids.add(step.kind.id);
    }
    if (step.kind.type === "quotaResumed") {
      const provider = step.kind.provider;
      // Old reset notices have no transition id: match their original request and provider,
      // before its confirmed recovery. Later resets and other requests remain historical rows.
      if (step.requestId != null && transitions.some((confirmed) => confirmed.requestId === step.requestId
        && confirmed.kind.type === "providerTransition" && confirmed.kind.to.provider === provider
        && confirmed.kind.startedAtMs >= (step.atMs ?? Infinity))) return false;
    }
    return true;
  });
}

export function leadTransitions(steps: readonly BlockOrchestratorStep[]): ProviderTransition[] {
  return transitionSteps(steps).flatMap((step) => step.kind.type === "providerTransition" && !step.kind.taskId ? [step.kind] : []);
}

/** A seam attached to this request is replaced only by its confirmed cross-provider move. */
export function transitionCoversSeam(transition: ProviderTransition, seam: BlockSeam): boolean {
  return !transition.taskId && transition.from.provider !== transition.to.provider && transition.to.provider === seam.movedTo;
}

/** Generic backend fallbacks describe account ownership, not a recorded historical name. */
export function transitionAccountLabel(transition: ProviderTransition): string | null {
  const label = transition.accountLabel.trim();
  return !label || /^(?:this computer['’]?s login|a new account)$/i.test(label) ? null : label;
}


/**
 * What the composer shows of a started session's model; while it holds an older CLI, how many
 * switch rows to the new one there were when it began holding.
 */
export type ShownChoice = { shown: ModelChoice; rowsBefore: number | null };

const same = (a: ModelChoice, b: ModelChoice) =>
  a.provider === b.provider && a.model === b.model && a.effort === b.effort && a.account === b.account;

/**
 * A move to another CLI shows in the composer together with its switch row, not when the move is
 * first recorded (goal 1): the CLI shown last is held until a new switch row confirms the new
 * one, or until the session stops running (a move that failed: what its next turn starts on
 * shows). `prev` comes back when nothing changed.
 */
export function nextShown(
  prev: ShownChoice | null,
  current: ModelChoice,
  transitions: readonly ProviderTransition[],
  running: boolean,
): ShownChoice {
  if (!prev || prev.shown.provider === current.provider || !running) {
    return prev && prev.rowsBefore === null && same(prev.shown, current) ? prev : { shown: current, rowsBefore: null };
  }
  const rows = transitions.filter((transition) => transition.to.provider === current.provider).length;
  if (prev.rowsBefore === null) return { shown: prev.shown, rowsBefore: rows };
  return rows > prev.rowsBefore ? { shown: current, rowsBefore: null } : prev;
}

/** The words Brigadier resumes a worker with after a limit (`workers/quota.rs`, `retry_worker_quota`). */
const QUOTA_RESUME = "[Quota is available again.";

/**
 * Where a worker's panel shows its confirmed switches, each once: in place of the message it was
 * resumed with, in order (`at`, by entry key); one with no such message after its latest work.
 */
export function placeSwitches(transitions: readonly ProviderTransition[], entries: readonly ThreadEntry[]) {
  const resumes = entries.flatMap((entry) =>
    entry.kind === "item" && entry.item.kind === "message" && entry.item.role === "user" && entry.item.text.startsWith(QUOTA_RESUME) ? [entry.item.key] : []);
  const at = new Map<string, ProviderTransition>();
  resumes.forEach((key, index) => {
    const transition = transitions[index];
    if (transition) at.set(key, transition);
  });
  return { at, rest: transitions.slice(resumes.length) };
}
