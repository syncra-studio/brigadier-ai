import type { FC } from "react";

import { useWorkersStrip, WorkersStrip } from "@/app/conversation/activity/WorkersStrip";
import { ComposerRailItem } from "@/components/conversation";

/**
 * The slot above the composer for live work (SESSION-FIXES-PLAN.md Q8): the Workers strip,
 * Brigadier's workers and the CLI's own subagents alike (D50), shown only while it has something. Its own commands show none: its open work block says what it waits on (D24).
 */
export const ComposerStatusSlot: FC<{ conversationId: string }> = ({ conversationId }) => {
  const workers = useWorkersStrip(conversationId);
  return (
    <>
      {workers && (
        <ComposerRailItem label="Live work">
          <div data-slot="composer-status-slot">
            <WorkersStrip conversationId={conversationId} view={workers} />
          </div>
        </ComposerRailItem>
      )}
    </>
  );
};
