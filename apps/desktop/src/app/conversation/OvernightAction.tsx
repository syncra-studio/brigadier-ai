import type { ReactNode } from "react";

import { useCardKeys } from "@/app/conversation/cardKeys";
import { revealOvernight } from "@/app/conversation/summaryState";
import { Button } from "@/components/controls";
import { useBoard } from "@/state/board";

/** An overnight proposal: the rail points to its card, where Start is. */
export function OvernightAction({
  id,
  onDismiss,
  footer,
}: {
  id: string;
  onDismiss: () => void;
  footer: ReactNode;
}) {
  const run = useBoard((s) => s.board?.overnight[id]);
  useCardKeys(Boolean(run), {
    enter: () => revealOvernight(id),
    escape: onDismiss,
  });
  if (!run) return null;
  return (
    <div className="flex flex-col gap-2">
      <div className="min-h-row flex min-w-0 items-center gap-2 px-3">
        <Button
          type="button"
          variant="link"
          size="sm"
          className="h-auto min-w-0 flex-1 justify-start px-0 text-start"
          onClick={() => revealOvernight(id)}
        >
          <span className="min-w-0 truncate" title={run.name}>
            View plan: {run.name}
          </span>
        </Button>
        <Button type="button" size="xs" variant="ghost" className="shrink-0" onClick={onDismiss}>
          Later
        </Button>
      </div>
      {footer}
    </div>
  );
}
