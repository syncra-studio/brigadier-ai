import type { FC } from "react";

import { type BlockSeam, seamMove } from "@/app/conversation/blocks";
import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import { ROW } from "@/components/conversation";
import { LabeledDivider } from "@/components/shared";

/** "Continued on Codex" where a thread went on in another CLI (S38c); nothing for a hand-off. */
export const SeamDivider: FC<{ seam: BlockSeam }> = ({ seam }) => {
  const movedTo = seamMove(seam);
  return movedTo ? (
    <LabeledDivider data-slot="seam" role="status">
      Continued on {PROVIDER_LABELS[movedTo]}
    </LabeledDivider>
  ) : null;
};

/**
 * A quiet line of what a worker did ("Worker "…" finished · …"), or of a background job's end
 * ("Background command finished · exit 0", the CLI's own words on hover).
 */
export const TaskNoticeRow: FC<{ text: string; detail?: string | undefined }> = ({ text, detail }) => (
  <div data-slot="task-notice" className={ROW} title={detail ?? text}>
    <span className="min-w-0 truncate">{text}</span>
  </div>
);
