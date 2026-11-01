import type { BlockOrchestratorStep, BlockSeam } from "@/app/conversation/blocks";
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


/** What the composer shows of a started session's model, and since when it holds an older one. */
export type ShownChoice = { shown: ModelChoice; holdingSinceMs: number | null };

/**
 * A move to another CLI shows in the composer together with its switch row, not when the move is
 * first recorded (goal 1): the CLI shown last is held until a switch row confirms the new one, or
 * until the session stops running (a move that failed: what its next turn starts on shows).
 */
export function nextShown(
  prev: ShownChoice | null,
  current: ModelChoice,
  transitions: readonly ProviderTransition[],
  running: boolean,
  nowMs: number,
): ShownChoice {
  if (!prev || prev.shown.provider === current.provider || !running) return { shown: current, holdingSinceMs: null };
  const since = prev.holdingSinceMs ?? nowMs;
  // The row can be confirmed a moment before the setup's event reaches the app.
  const confirmed = transitions.some((transition) => transition.to.provider === current.provider
    && transition.startedAtMs >= since - CONFIRM_SLACK_MS);
  return confirmed ? { shown: current, holdingSinceMs: null } : { shown: prev.shown, holdingSinceMs: since };
}

const CONFIRM_SLACK_MS = 5_000;
