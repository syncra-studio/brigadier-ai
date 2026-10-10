import { ApprovalCardView } from "@/app/conversation/cards/ApprovalCardView";
import { PlanCardLink } from "@/app/conversation/cards/PlanCardLink";
import { PlanDocCard, WritingPlanCard } from "@/app/conversation/cards/PlanDocument";
import { QuestionCardView } from "@/app/conversation/cards/QuestionCardView";
import { useBoard } from "@/state/board";

/**
 * What a thread item shows: a card of the board, a lead's outline the user is asked to start
 * (`outline`, shown as its plan), or "Writing plan" while the thread writes its plan
 * (`writingPlan`).
 */
export type CardType = "task" | "approval" | "question" | "plan" | "outline" | "writingPlan";

/** A plan with a body reads as a document; one of phases alone keeps its row. */
function PlanCard({ id }: { id: string }) {
  const document = useBoard((s) => !!s.board?.plans[id]?.body);
  return document ? <PlanDocCard docRef={{ type: "plan", id }} /> : <PlanCardLink cardId={id} />;
}

/** The card a thread item shows. Loaded with the first card, not at startup. */
export default function CardBody({ type, id }: { type: CardType; id: string }) {
  switch (type) {
    case "approval":
      return <ApprovalCardView cardId={id} />;
    case "question":
      return <QuestionCardView cardId={id} />;
    case "plan":
      return <PlanCard id={id} />;
    case "outline":
      return <PlanDocCard docRef={{ type: "outline", id }} />;
    case "writingPlan":
      return <WritingPlanCard />;
  }
}
