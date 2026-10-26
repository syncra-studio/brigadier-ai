import type { BlockOrchestratorStep, BlockSeam } from "@/app/conversation/blocks";
import type { OrchestratorStep } from "@/ipc/generated";

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

