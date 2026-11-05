import type { Activity, LeadStep } from "@/app/conversation/activity/group";
import { foldsAway, type SequenceEntry as Entry } from "@/app/conversation/blocks";

/**
 * A turn's work cut where a new turn of its CLI's joined it: runs of work, and what came between
 * them, the user's follow-up bubble or a line of what a worker did.
 */
export type WorkPart = { type: "work"; items: Activity<LeadStep, Entry>[]; answer?: Entry; kept?: Entry[] } | { type: "between"; entry: Entry };

/**
 * Whether an entry cuts the work: a follow-up steered in, a compaction's divider, where a turn
 * of it failed, or a line of what a worker did, unless it came with a turn that joined the block
 * that started the worker (that is the same work).
 */
function cuts(entry: Entry): boolean {
  return entry.kind === "steer" || entry.kind === "compaction" || entry.kind === "seam" || entry.kind === "turnError" || (entry.kind === "taskNotice" && !entry.inWork);
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
    // A compaction mid-turn is no new turn, nor is a failure a follow-up: the reply before it folds with the work.
    if (part.type !== "work" || next?.type !== "between" || next.entry.kind === "compaction" || next.entry.kind === "seam" || next.entry.kind === "turnError") return part;
    const last = part.items.at(-1);
    if (last?.type !== "entry" || last.entry.kind !== "text" || !answers(last.entry.index)) return part;
    return { ...part, items: part.items.slice(0, -1), answer: last.entry };
  });
}

/**
 * The work in runs between its cuts. A live turn always ends on a run, where its live line goes,
 * even right after a cut.
 */
function workParts(items: readonly Activity<LeadStep, Entry>[], live: boolean, stays: (entry: Entry) => boolean = () => false): WorkPart[] {
  const parts: WorkPart[] = [];
  for (const item of items) {
    if (item.type === "entry" && cuts(item.entry)) {
      parts.push({ type: "between", entry: item.entry });
      continue;
    }
    let last = parts.at(-1);
    if (last?.type !== "work") parts.push((last = { type: "work", items: [] }));
    // What stays in view of a folded run (a switch's divider, a kept card) stays with its run,
    // never below a follow-up that came after it (D65: no row moves when the turn ends).
    if (item.type === "entry" && stays(item.entry)) (last.kept ??= []).push(item.entry);
    else last.items.push(item);
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
  // The answer and what stays in view after it are drawn under the parts; the rest that stays in view, in its run.
  const below = new Set(underAnswer(activity, answer, done));
  const folded = activity.filter((item) => item.type === "group" || (!below.has(item.entry) && (item.entry.kind !== "text" || foldsAway(item.entry, answer))));
  return keepJoinedAnswers(done ? workParts(folded, false, (entry) => !foldsAway(entry, answer)) : workParts(activity, live), done, answers);
}

/**
 * What a finished turn shows under its answer, in the order it came: its update lines, and the
 * lines that stay in view written after the answer (a worker resumed or ended once the thread
 * replied), never above the reply that came before them.
 */
export function belowAnswer(activity: readonly Activity<LeadStep, Entry>[], answer: number | null, done: boolean): Entry[] {
  return underAnswer(activity, answer, done).filter((entry) => !isSwitch(entry));
}

/** A switch made after the answer: under its action row, which belongs to the answer above it. */
export function belowActions(activity: readonly Activity<LeadStep, Entry>[], answer: number | null, done: boolean): Entry[] {
  return underAnswer(activity, answer, done).filter(isSwitch);
}

const isSwitch = (entry: Entry) => entry.kind === "orchestrator" && entry.steps.some((step) => step.kind.type === "providerTransition");

function underAnswer(activity: readonly Activity<LeadStep, Entry>[], answer: number | null, done: boolean): Entry[] {
  if (!done || answer === null) return [];
  const at = activity.find((item) => item.type === "entry" && item.entry.kind === "text" && item.entry.index === answer);
  if (at?.type !== "entry") return [];
  return activity.flatMap((item) => (item.type === "entry" && item.entry.position > at.entry.position && !foldsAway(item.entry, answer) ? [item.entry] : []));
}
