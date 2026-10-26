import type { FC } from "react";

import { ChecksStrip, SubagentsStrip, useChecksStrip, useSubagentsStrip } from "@/app/conversation/activity/SessionStrips";
import { useWorkersStrip, WorkersStrip } from "@/app/conversation/activity/WorkersStrip";
import { ComposerRailItem } from "@/components/conversation";

/**
 * The slot above the composer for live work (SESSION-FIXES-PLAN.md Q8): the Workers strip
 * (Brigadier's workers), the session CLI's own Subagents, then its Checks (tests, builds,
 * waits) nearest the composer, where what the session waits on is read first. Each shows only
 * while it has something.
 */
export const ComposerStatusSlot: FC<{ conversationId: string }> = ({ conversationId }) => {
  const workers = useWorkersStrip(conversationId);
  const subagents = useSubagentsStrip(conversationId);
  const checks = useChecksStrip(conversationId);
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
      {checks && (
        <ComposerRailItem label="Checks">
          <ChecksStrip conversationId={conversationId} view={checks} />
        </ComposerRailItem>
      )}
    </>
  );
};
