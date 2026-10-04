import { revealPlan } from "@/app/conversation/summaryState";
import { Button } from "@/components/ui/button";
import type { PlanState } from "@/ipc/generated";
import { useBoard } from "@/state/board";

const STATE_WORDS: Record<PlanState["type"], string> = {
  proposed: "waiting for approval",
  approved: "approved",
  rejected: "rejected",
  superseded: "replaced",
};

/**
 * The plan's one row in the thread history: "View plan: Phase 2 · Fix · approved", which opens
 * the one actionable card under the summary.
 */
export function PlanCardLink({ cardId }: { cardId: string }) {
  const plan = useBoard((s) => s.board?.plans[cardId]);
  if (!plan) return null;
  return (
    <div data-slot="plan-row" className="flex flex-col">
      <div className="text-muted-foreground flex min-h-row-sm min-w-0 items-center gap-1.5 text-sm">
        <Button
          type="button"
          variant="link"
          size="sm"
          title={plan.title}
          className="h-auto min-w-0 shrink justify-start px-0 text-start"
          onClick={() => revealPlan(plan.id)}
        >
          <span className="min-w-0 truncate">View plan: {plan.title}</span>
        </Button>
        <span aria-hidden>·</span>
        <span className="shrink-0 whitespace-nowrap">{STATE_WORDS[plan.state.type]}</span>
      </div>
    </div>
  );
}
