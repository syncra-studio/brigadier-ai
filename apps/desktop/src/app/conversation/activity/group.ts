import { type BlockOrchestratorStep, isImageStep, type SequenceEntry } from "@/app/conversation/blocks";
import { isPlumbing } from "@/app/conversation/activity/words";
import type { ThreadEntry } from "@/components/transcript/activity";

/**
 * How a thread's work is grouped (THREAD-UX-PLAN.md §3.2–§3.3), the same for the lead's turn and
 * a worker's thread: consecutive work steps make one group, and only a reply, a team sentence, a
 * notice or a card ends it. Thinking is never a row and never ends a group: its words only feed
 * the live line while it streams. The live and the finished turn group the same way, so nothing
 * moves when the turn ends.
 */

export type GroupItem<S> = { type: "step"; key: string; step: S };

/** One item of a thread's activity: a group of work, or anything else as it is. */
export type Activity<S, E> = { type: "group"; key: string; items: GroupItem<S>[] } | { type: "entry"; entry: E };

/** What an entry is to the grouping. */
export type Sorted<S> =
  | { type: "step"; key: string; step: S }
  /** Ends the open group and shows as itself. */
  | { type: "break" }
  /** Not shown at all. */
  | { type: "skip" };

/** A thread's entries as its activity. */
export function groupActivity<S, E>(entries: readonly E[], sort: (entry: E) => Sorted<S>): Activity<S, E>[] {
  const out: Activity<S, E>[] = [];
  let open: Extract<Activity<S, E>, { type: "group" }> | null = null;
  for (const entry of entries) {
    const sorted = sort(entry);
    if (sorted.type === "skip") continue;
    if (sorted.type === "break") {
      open = null;
      out.push({ type: "entry", entry });
      continue;
    }
    if (!open) {
      open = { type: "group", key: sorted.key, items: [] };
      out.push(open);
    }
    open.items.push(sorted);
  }
  return out;
}

/** A heading line a model writes over a part of its thinking: `**Checking the tests**`. */
const HEADING = /^\s*\*\*(.+?)\*\*\s*$/;

/**
 * What the thread thinks about now, for its live line: the last whole line of its thinking (a
 * lone bold heading gives its words), without markdown; "" when no line is whole yet, and the
 * line says "Thinking". A line still streaming is never shown, so the label never reads half a
 * sentence.
 */
export function thinkingLabel(text: string, complete = false): string {
  const lines = text.split("\n");
  // The last line is whole once a newline follows it, or the thought is complete.
  const last = (complete ? lines : lines.slice(0, -1)).filter((line) => line.trim()).at(-1) ?? "";
  const heading = HEADING.exec(last)?.[1]?.trim();
  if (heading) return heading;
  return last.replace(/[*_`#>]+/g, "").replace(/\s+/g, " ").trim();
}

/** Whether the lead's step is work it did itself (a group's step), rather than news of its team. */
function leadWork(step: BlockOrchestratorStep): "work" | "hidden" | "apart" {
  const { kind } = step;
  switch (kind.type) {
    case "tool":
      return isPlumbing(kind.name) ? "hidden" : "work";
    case "searchedWeb":
    case "readPage":
      return "work";
    case "readArtifact":
      // The diff of a worker's change: the team sentence about it covers it.
      return kind.name.startsWith("Diff of") ? "hidden" : "work";
    case "readReport":
      return "hidden";
    // A message to another session is a row of the work (a wait for its reply joins it), its
    // refusal too.
    case "peerSent":
    case "messageRefused":
      return "work";
    default:
      return "apart";
  }
}

/** A lead's step in a group. */
export type LeadStep = BlockOrchestratorStep;

/**
 * A turn's activity: its sequence (from `blockSequence`) grouped. The live thought at the end of
 * a live turn is the live line's, not the activity's.
 */
export function turnActivity(sequence: readonly SequenceEntry[], joinPeers = true): Activity<LeadStep, SequenceEntry>[] {
  const grouped = groupLead(sequence);
  if (!joinPeers) return grouped;
  return grouped.map((item) => (item.type === "group" ? { ...item, items: joinExchanges(item.items) } : item));
}

/**
 * Messages to other sessions and the waits for their replies, next to each other in a group,
 * read as one row: "Sent messages to Docs and API work, waited for their replies".
 * A trailing send still in flight stays separate until its wait arrives or the turn ends.
 */
export function joinExchanges(items: GroupItem<LeadStep>[], sending: LeadStep | null = null): GroupItem<LeadStep>[] {
  const out: GroupItem<LeadStep>[] = [];
  for (const item of items) {
    const last = out.at(-1);
    if (item.step !== sending && item.type === "step" && item.step.kind.type === "peerSent" && last?.type === "step" && last.step.kind.type === "peerSent") {
      const kind = last.step.kind;
      const next = item.step.kind;
      out[out.length - 1] = {
        ...last,
        step: { ...last.step, kind: { ...kind, messageIds: [...kind.messageIds, ...next.messageIds], waits: [...kind.waits, ...next.waits] } },
      };
      continue;
    }
    out.push(item);
  }
  return out;
}

function groupLead(sequence: readonly SequenceEntry[]): Activity<LeadStep, SequenceEntry>[] {
  return groupActivity<LeadStep, SequenceEntry>(sequence, (entry) => {
    switch (entry.kind) {
      // Thinking is never a row: the live one feeds the live line.
      case "thinking":
        return { type: "skip" };
      case "orchestrator": {
        const [step] = entry.steps;
        // An image the thread looked at or made is a row of its own that ends the group.
        if (!step || entry.steps.length > 1 || isImageStep(step)) return { type: "break" };
        const work = leadWork(step);
        if (work === "hidden") return { type: "skip" };
        return work === "work" ? { type: "step", key: `step:${step.position}`, step } : { type: "break" };
      }
      default:
        return { type: "break" };
    }
  });
}

type ActionItem = Extract<ThreadEntry, { kind: "actions" }>["items"][number];

/** A worker's call on its own plumbing: the report is its answer. */
function workerPlumbing(item: ActionItem): boolean {
  return item.kind === "tool" && (isPlumbing(item.name) || item.name === "ToolSearch");
}

/**
 * The CLI's start and exit in a worker's thread: plumbing, except the exit of a worker that
 * failed. Brigadier ends a finished worker's CLI itself, often with a non-zero code.
 */
export function workerCliNotice(entry: ThreadEntry, failed: boolean): boolean {
  if (entry.kind !== "item" || entry.item.kind !== "notice" || !entry.item.cli) return false;
  return entry.item.cli === "started" || !failed;
}

/** A worker's thread's activity: its transcript's entries, grouped the same way. */
export function workerActivity(entries: readonly ThreadEntry[]): Activity<ActionItem, ThreadEntry>[] {
  type Flat = ThreadEntry | { kind: "action"; item: ActionItem };
  const flat = entries.flatMap((entry): Flat[] =>
    entry.kind === "actions" ? entry.items.map((item) => ({ kind: "action" as const, item })) : [entry],
  );
  return groupActivity(flat, (entry) => {
    if (entry.kind === "action") {
      return workerPlumbing(entry.item) ? { type: "skip" } : { type: "step", key: entry.item.key, step: entry.item };
    }
    // An answered approval leaves no trace: the action's own row tells what came of it.
    if (entry.kind === "item" && entry.item.kind === "approval" && entry.item.resolution) return { type: "skip" };
    // Thinking is never a row: the live one feeds the live line.
    if (entry.kind === "item" && entry.item.kind === "reasoning") return { type: "skip" };
    return { type: "break" };
  }) as Activity<ActionItem, ThreadEntry>[]; // An action is always a step or skipped, never an entry.
}
