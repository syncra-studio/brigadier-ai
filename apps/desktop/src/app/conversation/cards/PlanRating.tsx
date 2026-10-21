import { RateItem, RateMenu } from "@/components/conversation";
import { useAction } from "@/app/conversation/useAction";
import type { Rating } from "@/ipc/generated";
import { rateMessage } from "@/state/actions";
import { useBoard } from "@/state/board";

/** "Rate this plan": Good plan / Bad plan, and Clear rating once rated; kept on this machine like an answer's (#127). */
export function PlanRating({ conversationId, planId }: { conversationId: string; planId: string }) {
  const subject = `plan:${planId}`;
  const rated = useBoard((s) => s.board?.ratings[subject] ?? null);
  const action = useAction();
  const rate = (rating: Rating | null) => action.run(() => rateMessage(conversationId, subject, rating));
  return (
    <RateMenu rated={rated} onClear={() => rate(null)} tooltip="Rate this plan" below className="text-foreground/50 hover:text-foreground size-6 shrink-0">
      <RateItem rating="good" of="plan" onSelect={() => rate("good")} />
      <RateItem rating="bad" of="plan" onSelect={() => rate("bad")} />
    </RateMenu>
  );
}
