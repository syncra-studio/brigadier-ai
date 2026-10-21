import type { ReactNode } from "react";

import { ImplementPlanAction } from "@/app/conversation/ImplementPlanAction";
import { PhasePlanAction } from "@/app/conversation/PhasePlanAction";
import { useBoard } from "@/state/board";

/**
 * A proposed plan: one written as a document asks "Implement this plan?"; one of phases alone
 * keeps its rail link to the plan card, with message entry available.
 */
export function PlanAction({
  id,
  onDismiss,
  footer,
  stagger,
}: {
  id: string;
  onDismiss: () => void;
  footer: ReactNode;
  stagger: boolean;
}) {
  const document = useBoard((s) => !!s.board?.plans[id]?.body);
  return document ? (
    <ImplementPlanAction docRef={{ type: "plan", id }} onDismiss={onDismiss} footer={footer} stagger={stagger} />
  ) : (
    <PhasePlanAction id={id} onDismiss={onDismiss} footer={footer} />
  );
}
