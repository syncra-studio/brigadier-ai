import type { FC } from "react";

import { ROW } from "@/components/conversation";

/** The CLI's line for a background job's end ("Background command … completed"), a quiet line before its reply. */
export const TaskNoticeRow: FC<{ text: string }> = ({ text }) => (
  <div data-slot="task-notice" className={ROW} title={text}>
    <span className="min-w-0 truncate">{text}</span>
  </div>
);
