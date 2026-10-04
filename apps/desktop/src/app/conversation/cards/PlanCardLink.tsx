import { ChevronRight } from "@openai/apps-sdk-ui/components/Icon";
import { useShallow } from "zustand/react/shallow";

import { checkersOf, checkRounds } from "@/app/conversation/rowWords";
import { revealPlan } from "@/app/conversation/summaryState";
import { ChecksList } from "@/app/conversation/TaskRow";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import type { Gate, Plan, PlanState } from "@/ipc/generated";
import { type Board, useBoard } from "@/state/board";

/** The plan's revisions, oldest first, ending with `plan` itself. */
function revisionsOf(board: Board | null | undefined, plan: Plan | undefined): Plan[] {
  const chain: Plan[] = [];
  const seen = new Set<string>();
  for (let at = plan; at && !seen.has(at.id); at = at.revises ? board?.plans[at.revises] : undefined) {
    seen.add(at.id);
    chain.unshift(at);
  }
  return chain;
}

const STATE_WORDS: Record<PlanState["type"], string> = {
  proposed: "waiting for approval",
  inReview: "in review",
  revising: "being revised",
  approved: "approved",
  rejected: "rejected",
  superseded: "revised",
};

function times(count: number): string {
  return count === 1 ? "once" : count === 2 ? "twice" : `${count} times`;
}

/**
 * The plan's one row in the thread history: "View plan: Phase 2 · Fix · approved · reviewed
 * twice, 4 notes", which opens the one actionable card under the summary; the chevron opens its
 * review rounds, each reviewer openable. A revision replaces the plan it revises here.
 */
export function PlanCardLink({ cardId }: { cardId: string }) {
  const plan = useBoard((s) => s.board?.plans[cardId]);
  const current = useBoard((s) => {
    if (!plan || !s.board) return null;
    // A superseded revision opens its latest successor, whose review retains the findings.
    let latest = plan;
    const seen = new Set<string>();
    while (!seen.has(latest.id)) {
      seen.add(latest.id);
      const next = Object.values(s.board.plans).find(
        (candidate) => candidate.revises === latest.id,
      );
      if (!next) break;
      latest = next;
    }
    return latest.id;
  });
  const revisions = useBoard(useShallow((s) => revisionsOf(s.board, plan)));
  const owners = revisions.map((revision) => `plan:${revision.id}`);
  const checkerIds = useBoard(
    useShallow((s) => (s.board ? checkersOf(s.board.tasks, owners).map((task) => task.id) : [])),
  );
  const rounds = useBoard((s) => (s.board ? checkRounds(checkersOf(s.board.tasks, owners)).length : 0));
  if (!plan || !current) return null;
  const notes = plan.reviewNotes.length;
  const detail = [
    STATE_WORDS[plan.state.type],
    rounds > 0 && `reviewed ${times(rounds)}${notes > 0 ? `, ${notes} ${notes === 1 ? "note" : "notes"}` : ""}`,
  ]
    .filter(Boolean)
    .join(" · ");
  const gates: Record<string, Gate | null> = Object.fromEntries(
    revisions.map((revision) => [`plan:${revision.id}`, revision.gate]),
  );
  return (
    <Collapsible data-slot="plan-row" className="flex flex-col">
      <div className="text-muted-foreground flex min-h-row-sm min-w-0 items-center gap-1.5 text-sm">
        <Button
          type="button"
          variant="link"
          size="sm"
          title={plan.title}
          className="h-auto min-w-0 shrink justify-start px-0 text-start"
          onClick={() => revealPlan(current)}
        >
          <span className="min-w-0 truncate">View plan: {plan.title}</span>
        </Button>
        <span aria-hidden>·</span>
        <span className="shrink-0 whitespace-nowrap">{detail}</span>
        {checkerIds.length > 0 && (
          <CollapsibleTrigger
            aria-label="Show the plan's reviews"
            className="group/opener hover:text-foreground rounded-control focus-visible:ring-ring/50 flex size-control-xs shrink-0 items-center justify-center outline-none focus-visible:ring-1"
          >
            <ChevronRight
              aria-hidden
              className="size-icon-xs transition-[rotate] group-data-[state=open]/opener:rotate-90 motion-reduce:transition-none"
            />
          </CollapsibleTrigger>
        )}
      </div>
      <CollapsibleContent className="text-muted-foreground ps-6 pt-1 pb-1 text-sm">
        <ChecksList checkerIds={checkerIds} gates={gates} />
      </CollapsibleContent>
    </Collapsible>
  );
}
