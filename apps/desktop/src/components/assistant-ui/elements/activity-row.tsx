import {
  Book,
  CheckCircle,
  EditPencil,
  Globe,
  PlayCircle,
  Search,
  Sparkle,
  Terminal,
  Tools,
} from "@openai/apps-sdk-ui/components/Icon";
import type { FC } from "react";

import type { WorkKind } from "@/app/conversation/activity/words";

/**
 * The one row recipe of a thread's activity (THREAD-UX-PLAN.md §3.7), the lead's and a worker's
 * alike: work groups, their steps, team sentences and notices.
 */
export const ROW = "text-foreground/60 flex min-h-5 min-w-0 items-center gap-1.5 text-sm leading-5";

/** What a row opens to: flush under it, in the same type. */
export const ROW_DETAIL = "text-foreground/60 flex max-h-action-list min-w-0 flex-col gap-1 overflow-y-auto ps-5.5 pt-1 pb-1 text-sm";

/** The chevron of a row that opens: always shown, turned while open. */
export const CHEVRON =
  "text-foreground/40 size-icon-xs shrink-0 transition-[rotate] group-data-[state=open]:rotate-90 motion-reduce:transition-none";

export const WORK_ICONS: Record<WorkKind, FC<{ className?: string }>> = {
  read: Book,
  edit: EditPencil,
  run: Terminal,
  code: Search,
  web: Globe,
  memory: Sparkle,
  checks: CheckCircle,
  preview: PlayCircle,
  other: Tools,
};
