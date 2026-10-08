import { ApprovalCardView } from "@/app/conversation/cards/ApprovalCardView";
import { PlanCardLink } from "@/app/conversation/cards/PlanCardLink";
import { QuestionCardView } from "@/app/conversation/cards/QuestionCardView";

export type CardType = "task" | "approval" | "question" | "plan";

/** The card a thread item shows. Loaded with the first card, not at startup. */
export default function CardBody({ type, id }: { type: CardType; id: string }) {
  switch (type) {
    case "approval":
      return <ApprovalCardView cardId={id} />;
    case "question":
      return <QuestionCardView cardId={id} />;
    case "plan":
      return <PlanCardLink cardId={id} />;
  }
}
