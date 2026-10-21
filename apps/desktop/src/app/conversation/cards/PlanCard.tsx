import { PlanCardLink } from "@/app/conversation/cards/PlanCardLink";
import { PlanDocCard } from "@/app/conversation/cards/PlanDocument";
import { useBoard } from "@/state/board";

/** A plan with a body reads as a document; one of phases alone keeps its row. */
export function PlanCard({ id }: { id: string }) {
  const document = useBoard((s) => !!s.board?.plans[id]?.body);
  return document ? <PlanDocCard docRef={{ type: "plan", id }} /> : <PlanCardLink cardId={id} />;
}
