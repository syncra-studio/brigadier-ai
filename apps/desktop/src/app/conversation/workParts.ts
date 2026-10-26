import type { Activity, LeadStep } from "@/app/conversation/activity/group";
import { foldsAway, type SequenceEntry as Entry } from "@/app/conversation/blocks";

/**
 * A turn's work cut where a new turn of its CLI's joined it: runs of work, and what came between
 * them, the user's follow-up bubble or a line of what a worker did.
 */
export type WorkPart = { type: "work"; items: Activity<LeadStep, Entry>[]; answer?: Entry } | { type: "between"; entry: Entry };

/**
 * Whether an entry cuts the work: a follow-up steered in, a compaction's divider, or a line of
 * what a worker did, unless it came with a turn that joined the block that started the worker
 * (that is the same work).
 */
function cuts(entry: Entry): boolean {
  return entry.kind === "steer" || entry.kind === "compaction" || (entry.kind === "taskNotice" && !entry.inWork);
}

/**
 * A finished turn another joined (a follow-up sent while it ended, or a background job's end the
 * CLI answered): the reply each run of work came to before the next cut is an answer of its own,
 * so it stays in view under its run's fold instead of folding away with the work. Only the
 * turn's own last answer stood outside the fold before.
 */
function keepJoinedAnswers(parts: WorkPart[], done: boolean, answers: (index: number) => boolean): WorkPart[] {
  if (!done) return parts;
  return parts.map((part, index) => {
    const next = parts[index + 1];
    // A compaction mid-turn is no new turn: the reply before it folds with the work.
    if (part.type !== "work" || next?.type !== "between" || next.entry.kind === "compaction") return part;
    const last = part.items.at(-1);
    if (last?.type !== "entry" || last.entry.kind !== "text" || !answers(last.entry.index)) return part;
    return { type: "work", items: part.items.slice(0, -1), answer: last.entry };
  });
}

/**
 * The work in runs between its cuts. A live turn always ends on a run, where its live line goes,
 * even right after a cut.
 */
function workParts(items: readonly Activity<LeadStep, Entry>[], live: boolean): WorkPart[] {
  const parts: WorkPart[] = [];
  for (const item of items) {
    if (item.type === "entry" && cuts(item.entry)) {
      parts.push({ type: "between", entry: item.entry });
      continue;
    }
    const last = parts.at(-1);
    if (last?.type === "work") last.items.push(item);
    else parts.push({ type: "work", items: [item] });
  }
  if (live && parts.at(-1)?.type !== "work") parts.push({ type: "work", items: [] });
  return parts;
}

/**
 * A turn's parts as the thread draws them: done, its work folds but for its answer (`answer`) and
 * what stays in view; `answers` says which replies are answers, not commentary or update lines.
 */
export function turnParts(
  activity: readonly Activity<LeadStep, Entry>[],
  answer: number | null,
  done: boolean,
  live: boolean,
  answers: (index: number) => boolean,
): WorkPart[] {
  const folded = activity.filter((item) => item.type === "group" || foldsAway(item.entry, answer));
  return keepJoinedAnswers(workParts(done ? folded : activity, live && !done), done, answers);
}
