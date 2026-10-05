import {
  Book,
  Chat,
  Commit,
  EditPencil,
  Folder,
  Globe,
  CheckCircle,
  Reply,
  Search,
  ShieldCheck,
  Sparkle,
  Terminal,
  Tools,
} from "@openai/apps-sdk-ui/components/Icon";
import type { FC } from "react";

import type { ToolKind } from "@/app/conversation/toolWords";

/** Quiet non-worker actions in the main conversation. */
export const ACTIVITY_ROW =
  "text-foreground/60 flex min-h-5 min-w-0 items-center gap-1.5 text-sm leading-5";
export const ACTIVITY_DETAIL =
  "text-foreground/60 flex max-h-action-list min-w-0 flex-col gap-2 overflow-y-auto ps-6 pt-2 pb-1 text-sm";
export const ACTIVITY_ICONS: Record<ToolKind, FC<{ className?: string }>> = {
  read: Book,
  search: Search,
  list: Folder,
  edit: EditPencil,
  run: Terminal,
  worker: Sparkle,
  message: Chat,
  memory: Book,
  plan: CheckCircle,
  approval: ShieldCheck,
  land: Commit,
  report: Reply,
  web: Globe,
  image: Sparkle,
  tool: Tools,
};
