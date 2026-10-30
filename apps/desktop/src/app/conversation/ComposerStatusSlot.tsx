import type { FC } from "react";

import { SubagentsStrip, useSubagentsStrip } from "@/app/conversation/activity/SessionStrips";
import { useWorkersStrip, WorkersStrip } from "@/app/conversation/activity/WorkersStrip";
import { ComposerRailItem } from "@/components/conversation";

/**
 * The slot above the composer for live work (SESSION-FIXES-PLAN.md Q8): the Workers strip
 * (Brigadier's workers), then the session CLI's own Subagents. Each shows only while it has
 * something. Its own commands show none: its open work block says what it waits on (D24).
 */
export const ComposerStatusSlot: FC<{ conversationId: string }> = ({ conversationId }) => {
  const workers = useWorkersStrip(conversationId);
  const subagents = useSubagentsStrip(conversationId);
  return (
    <>
      {workers && (
        <ComposerRailItem label="Live work">
          <div data-slot="composer-status-slot">
            <WorkersStrip conversationId={conversationId} view={workers} />
          </div>
        </ComposerRailItem>
      )}
      {subagents && (
        <ComposerRailItem label="Subagents">
          <SubagentsStrip conversationId={conversationId} view={subagents} />
        </ComposerRailItem>
      )}
    </>
  );
};
