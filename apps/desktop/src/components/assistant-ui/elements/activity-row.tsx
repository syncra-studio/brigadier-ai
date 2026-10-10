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
 * alike: work groups, their steps, team sentences and notices. As wide as its words (up to the
 * column, then they truncate), one 21px line, everything in it centred on that line.
 */
export const ROW =
  "text-foreground/60 flex w-fit max-w-full min-h-activity-row min-w-0 items-center gap-1.5 text-sm leading-(--spacing-activity-row)";

/** A row that opens: the whole row is the button, and its words turn white on hover or focus. */
export const ROW_TOGGLE = "group hover:text-foreground focus-visible:text-foreground rounded-control cursor-pointer text-start";

/** What a row opens to: flush under it, in the same type. */
export const ROW_DETAIL = "text-foreground/60 flex max-h-action-list min-w-0 flex-col gap-1 overflow-y-auto ps-5.5 pt-1 pb-1 text-sm";

/**
 * The chevron of a row that opens: always shown, quieter than the words, 4px after them and
 * centred on their line; white with the words on hover, turned down while open.
 */
export const CHEVRON =
  "text-row-chevron size-chevron -ms-0.5 shrink-0 transition-[rotate,color] duration-300 ease-standard group-hover:text-foreground group-focus-visible:text-foreground group-data-[state=open]:rotate-90 motion-reduce:transition-none";

/**
 * How a row's or group's detail opens and closes: its height and opacity over 160 ms, at once
 * with reduced motion. On the collapsible's content; the detail's own layout goes inside it.
 */
export const OPENS =
  "overflow-hidden data-[state=open]:animate-collapsible-down data-[state=closed]:animate-collapsible-up motion-reduce:animate-none";

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
