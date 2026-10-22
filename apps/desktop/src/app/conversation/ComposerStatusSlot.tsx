import type { FC } from "react";

import { useWorkersStrip, WorkersStrip } from "@/app/conversation/activity/WorkersStrip";
import { ComposerRailItem } from "@/components/conversation";

/**
 * The one slot above the composer for live work (SESSION-FIXES-PLAN.md Q8): the Workers strip.
 * CI is the Job card's (R2).
 */
export const ComposerStatusSlot: FC<{ conversationId: string }> = ({ conversationId }) => {
  const strip = useWorkersStrip(conversationId);
  if (!strip) return null;
  return (
    <ComposerRailItem label="Live work">
      <div data-slot="composer-status-slot">
        <WorkersStrip conversationId={conversationId} view={strip} />
      </div>
    </ComposerRailItem>
  );
};
