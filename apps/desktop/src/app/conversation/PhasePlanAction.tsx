import type { ReactNode } from "react";

import { PlanCardLink } from "@/app/conversation/cards/PlanCardLink";
import { useCardKeys } from "@/app/conversation/cardKeys";
import { useAction } from "@/app/conversation/useAction";
import { Button } from "@/components/controls";
import { useBoard } from "@/state/board";

/**
 * The rail points to the single plan card and keeps message entry available. The plan is
 * approved on its card, never by an Enter on the rail: Esc puts it off for later.
 */
export function PhasePlanAction({
  id,
  onDismiss,
  footer,
}: {
  id: string;
  onDismiss: () => void;
  footer: ReactNode;
}) {
  const plan = useBoard((s) => s.board?.plans[id]);
  const action = useAction();
  useCardKeys(Boolean(plan) && !action.busy, { escape: onDismiss });
  return (
    <div className="flex flex-col gap-2">
      <div className="min-h-row flex min-w-0 items-center gap-2 px-3">
        <div className="min-w-0 flex-1">
          <PlanCardLink cardId={id} />
        </div>
        <Button type="button" size="xs" variant="ghost" className="shrink-0" onClick={onDismiss}>
          Later
        </Button>
      </div>
      {action.error && (
        <p role="alert" className="text-destructive px-3 text-xs">
          {action.error}
        </p>
      )}
      {footer}
    </div>
  );
}
