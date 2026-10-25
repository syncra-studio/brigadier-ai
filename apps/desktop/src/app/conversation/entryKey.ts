import { stepKey } from "@/app/conversation/activity/group";
import type { SequenceEntry as Entry } from "@/app/conversation/blocks";

/**
 * A sequence entry's key: by the ids that name it where it has them (a call, a card, a notice's
 * item, a steer's message), so a reread that moves it keeps its row and what was opened in it
 * (D44); by its place otherwise.
 */
export function entryKey(entry: Entry): string {
  switch (entry.kind) {
    case "thinking":
      return `thinking:${entry.segment.itemId}`;
    case "text":
      return `text:${entry.index}`;
    case "card":
      return `${entry.card.type}:${entry.card.id}`;
    case "steer":
      return `steer:${entry.requestId ?? entry.position}`;
    case "orchestrator": {
      const [step] = entry.steps;
      return step ? `orchestrator:${stepKey(step)}` : `orchestrator:${entry.position}`;
    }
    case "compaction":
      return `compaction:${entry.compaction.id}`;
    case "nudge":
      return `nudge:${entry.position}`;
    case "taskNotice":
      return `task-notice:${entry.id ?? entry.position}`;
    case "row":
      return `task:${entry.row.taskId}:${entry.position}`;
    case "gap":
      return `gap:${entry.position}`;
  }
}
