import {
  Chat,
  EditPencil,
  File,
  Folder,
  Globe,
  MagnifyingGlassSmSearch,
  PlayCircle,
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
 * The chevron of a row that opens: hidden until the row is hovered or focused, 4px after the
 * words and centred on their line, white with them; turned down and kept while open.
 */
export const CHEVRON =
  "size-chevron -ms-0.5 shrink-0 opacity-0 transition-[rotate,color,opacity] duration-300 ease-standard group-hover:text-foreground group-hover:opacity-100 group-focus-visible:text-foreground group-focus-visible:opacity-100 group-data-[state=open]:rotate-90 group-data-[state=open]:opacity-100 motion-reduce:transition-none";

/**
 * How a row's or group's detail opens and closes: its height and opacity over 300 ms, fast out, at once
 * with reduced motion (`!`: the open and closed animations outrank a plain `motion-reduce:`). On the
 * collapsible's content; the detail's own layout goes inside it. It clips without scrolling, so a
 * sticky header inside sticks to the list that scrolls it.
 */
export const OPENS =
  "overflow-clip data-[state=open]:animate-collapsible-down data-[state=closed]:animate-collapsible-up motion-reduce:animate-none!";

/** A row's icon by its kind of work: a terminal for commands and checks, a file for reads, a magnifier for searches. */
export const WORK_ICONS: Record<WorkKind, FC<{ className?: string }>> = {
  read: File,
  edit: EditPencil,
  run: Terminal,
  code: MagnifyingGlassSmSearch,
  web: Globe,
  memory: Sparkle,
  checks: Terminal,
  preview: PlayCircle,
  message: Chat,
  other: Tools,
};

/** A listing's icon: a folder, where a search has the magnifier. */
export const LIST_ICON = Folder;
