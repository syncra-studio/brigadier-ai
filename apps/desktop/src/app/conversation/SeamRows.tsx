import type { FC } from "react";

import { type BlockSeam, seamMove } from "@/app/conversation/blocks";
import { ROW } from "@/components/conversation";
import { LabeledDivider } from "@/components/shared";
import { REBIRTH } from "@/app/side-panel/workerSeams";
import { cliFacts } from "@/lib/providerKinds";

/**
 * Where a thread went on: "Continued on Codex" in another CLI (S38c), else "Continued in a fresh
 * session" at a hand-off (core-goals D120), its note turn under the hood.
 */
export const SeamDivider: FC<{ seam: BlockSeam }> = ({ seam }) => {
  const movedTo = seamMove(seam);
  return (
    <LabeledDivider data-slot="seam" role="status">
      {movedTo ? `Continued on ${cliFacts(movedTo).label}` : REBIRTH}
    </LabeledDivider>
  );
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
