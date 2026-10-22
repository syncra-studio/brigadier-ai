import { ApprovalCardView } from "@/app/conversation/cards/ApprovalCardView";
import { QuestionCardView } from "@/app/conversation/cards/QuestionCardView";

/** What a thread item shows: a card of the board. */
export type CardType = "task" | "approval" | "question";

/** The card a thread item shows. Loaded with the first card, not at startup. */
export default function CardBody({ type, id }: { type: CardType; id: string }) {
  switch (type) {
    case "approval":
      return <ApprovalCardView cardId={id} />;
    case "question":
      return <QuestionCardView cardId={id} />;
  }
}
