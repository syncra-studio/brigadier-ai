import type { FC } from "react";

import type { BlockSeam } from "@/app/conversation/blocks";
import { useViewConversation } from "@/app/conversation/viewContext";
import { ROW } from "@/components/conversation";
import { LabeledDivider } from "@/components/shared";
import { openFileTab } from "@/state/sessionTabs";

/**
 * A seam (G12): "Fresh context · hand-off note", the note opening in a tab. A session that
 * ended with no note says why, and opens what its successor started from.
 */
export const SeamDivider: FC<{ seam: BlockSeam }> = ({ seam }) => {
  const conversationId = useViewConversation()?.id ?? null;
  const file = seam.note ?? seam.start;
  const label = seam.note ? "hand-off note" : `no hand-off note${seam.death ? ` (${seam.death})` : ""}`;
  return (
    <LabeledDivider data-slot="seam" role="status">
      <span className="min-w-0 text-center">
        {"Fresh context · "}
        {file && conversationId ? (
          <button type="button" className="hover:text-foreground focus-visible:text-foreground cursor-pointer underline-offset-2 hover:underline" onClick={() => openFileTab(conversationId, file)}>
            {label}
          </button>
        ) : (
          label
        )}
      </span>
    </LabeledDivider>
  );
};

/** The CLI's line for a background job's end ("Background command … completed"), a quiet line before its reply. */
export const TaskNoticeRow: FC<{ text: string }> = ({ text }) => (
  <div data-slot="task-notice" className={ROW} title={text}>
    <span className="min-w-0 truncate">{text}</span>
  </div>
);

/** Brigadier's one message at a seam, as its own quiet line in the turn. */
export const NudgeRow: FC = () => (
  <div data-slot="seam-nudge" className={ROW}>
    <span className="min-w-0 truncate">Brigadier asked for a hand-off note</span>
  </div>
);
