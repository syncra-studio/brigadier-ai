import type { ReactNode } from "react";

import { ChoiceCard } from "@/app/conversation/ChoiceCard";
import { useAction } from "@/app/conversation/useAction";
import { answerCard, decidePlan, putOffPlan } from "@/state/actions";
import { useBoard } from "@/state/board";
import { type PlanDocRef, planDoc } from "@/state/planDoc";

/**
 * "Implement this plan?" for a plan the user reads as a document: Yes builds it; typed words
 * send it back. A thread's own plan is decided as a plan (the thread revises it); a lead's
 * outline answers its approval (Yes starts the lead, words go to it as corrections). A plan's
 * "Not now" puts it off, which the thread answers in a line; an outline's Skip, and ×, put the
 * card aside.
 */
export function ImplementPlanAction({
  docRef,
  onDismiss,
  footer,
  stagger,
}: {
  docRef: PlanDocRef;
  onDismiss: () => void;
  footer: ReactNode;
  stagger: boolean;
}) {
  const conversationId = useBoard((s) => s.board?.conversationId ?? null);
  const shown = useBoard((s) => planDoc(s.board, docRef) !== null);
  const action = useAction();
  if (!shown || !conversationId) return null;
  const decide = (yes: boolean, text: string | null) =>
    action.run(() =>
      docRef.type === "plan"
        ? decidePlan(conversationId, docRef.id, yes, text)
        : answerCard(conversationId, docRef.id, yes ? { type: "allow" } : { type: "deny", message: text ?? "" }),
    );
  return (
    <ChoiceCard
      name="plan"
      title="Implement this plan?"
      choices={[{ label: "Yes, implement this plan" }]}
      placeholder="No, and tell Brigadier what to do differently"
      onChoose={() => decide(true, null)}
      onText={(text) => decide(false, text)}
      skipLabel={docRef.type === "plan" ? "Not now" : "Skip"}
      onSkip={docRef.type === "plan" ? () => action.run(() => putOffPlan(conversationId, docRef.id)) : onDismiss}
      onDismiss={onDismiss}
      busy={action.busy}
      error={action.error}
      footer={footer}
      stagger={stagger}
    />
  );
}
