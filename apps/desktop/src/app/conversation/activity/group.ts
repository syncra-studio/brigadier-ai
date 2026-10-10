import type { BlockOrchestratorStep, SequenceEntry } from "@/app/conversation/blocks";
import { isPlumbing } from "@/app/conversation/activity/words";
import type { ThinkingSegment } from "@/ipc/generated";
import type { ThreadEntry } from "@/components/transcript/activity";

/**
 * How a thread's work is grouped (THREAD-UX-PLAN.md §3.2–§3.3), the same for the lead's turn and
 * a worker's thread: consecutive work steps make one group, and only a reply, a team sentence, a
 * notice or a card ends it. Thinking never ends a group and is never a row of its own: it sits
 * inside a group, shown when the group is opened. The live and the finished turn group the same
 * way, so nothing moves when the turn ends.
 */

/**
 * A thought, as both threads keep one: its text and how long the provider reasoned, from its
 * own reasoning events (0 when that wasn't measured: a thought that arrived in one piece).
 */
export type Thought = { key: string; text: string; ms: number };

export type GroupItem<S> = { type: "step"; key: string; step: S } | { type: "thought"; thought: Thought };

/** One item of a thread's activity: a group of work, or anything else as it is. */
export type Activity<S, E> = { type: "group"; key: string; items: GroupItem<S>[] } | { type: "entry"; entry: E };

/** What an entry is to the grouping. */
export type Sorted<S> =
  | { type: "step"; key: string; step: S }
  | { type: "thought"; thought: Thought }
  /** Ends the open group and shows as itself. */
  | { type: "break" }
  /** Not shown at all. */
  | { type: "skip" };

/**
 * A thread's entries as its activity. A run of thoughts with no step goes into the next group of
 * the turn, else the one before it; with no group at all it is a group of its own.
 */
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
      open = { type: "group", key: sorted.type === "step" ? sorted.key : `thought:${sorted.thought.key}`, items: [] };
      out.push(open);
    }
    open.items.push(sorted.type === "step" ? { type: "step", key: sorted.key, step: sorted.step } : sorted);
  }
  return placeThoughts(out).map((item) => (item.type === "group" ? { ...item, items: joinThoughts(item.items) } : item));
}

/** Thoughts next to each other in a group read as one. */
function joinThoughts<S>(items: GroupItem<S>[]): GroupItem<S>[] {
  const out: GroupItem<S>[] = [];
  for (const item of items) {
    const last = out.at(-1);
    if (item.type === "thought" && last?.type === "thought") {
      const text = [last.thought.text, item.thought.text].filter((part) => part.trim()).join("\n\n");
      // Each one's own reasoning time, never the time between them.
      out[out.length - 1] = { type: "thought", thought: { ...last.thought, text, ms: last.thought.ms + item.thought.ms } };
      continue;
    }
    out.push(item);
  }
  return out;
}

function hasSteps<S, E>(item: Activity<S, E>): boolean {
  return item.type === "group" && item.items.some((inner) => inner.type === "step");
}

/**
 * Thought-only groups join the next group of work, else the one before. With no work at all (a
 * turn that only delegated), its thoughts are one row, where the first was.
 */
function placeThoughts<S, E>(items: Activity<S, E>[]): Activity<S, E>[] {
  if (!items.some((item) => hasSteps(item))) {
    const first = items.findIndex((item) => item.type === "group");
    if (first < 0) return items;
    const thoughts = items.flatMap((item) => (item.type === "group" ? item.items : []));
    return items.flatMap((item, index): Activity<S, E>[] =>
      item.type !== "group" ? [item] : index === first ? [{ ...item, items: thoughts }] : [],
    );
  }
  const out: Activity<S, E>[] = [];
  let carried: GroupItem<S>[] = [];
  for (const item of items) {
    if (item.type === "group" && !hasSteps(item)) {
      carried.push(...item.items);
      continue;
    }
    if (item.type === "group" && carried.length > 0) {
      out.push({ ...item, items: [...carried, ...item.items] });
      carried = [];
      continue;
    }
    out.push(item);
  }
  if (carried.length > 0) {
    const last = out.findLastIndex((item) => hasSteps(item));
    const group = out[last] as { type: "group"; key: string; items: GroupItem<S>[] };
    out[last] = { ...group, items: [...group.items, ...carried] };
  }
  return out;
}

/**
 * A finished thinking segment as a thought; one with no text isn't shown at all. Its time runs
 * from its first streamed piece to its last, so one that arrived whole has none.
 */
export function thoughtOf(segment: ThinkingSegment): Thought | null {
  if (!segment.text.trim()) return null;
  return { key: segment.itemId, text: segment.text, ms: Math.max(0, segment.updatedAtMs - segment.startedAtMs) };
}

/** A heading line a model writes over a part of its thinking: `**Checking the tests**`. */
const HEADING = /^\s*\*\*(.+?)\*\*\s*$/;

/**
 * What a thought is about, in one line: its first (or, while it streams, its newest) heading,
 * else its first sentence (live, the whole words of one still streaming); "" when it has no
 * words. The row truncates it to the line.
 */
export function thoughtTopic(text: string, which: "first" | "newest" = "first"): string {
  const lines = text.split("\n");
  const headings = lines.flatMap((line) => HEADING.exec(line)?.[1]?.trim() || []);
  const heading = which === "first" ? headings[0] : headings.at(-1);
  if (heading) return heading;
  const plain = lines
    .filter((line) => !HEADING.test(line))
    .join(" ")
    .replace(/[*_`#>]+/g, "")
    .replace(/\s+/g, " ")
    .trim();
  const sentence = /^.+?[.!?…](?=\s|$)/.exec(plain)?.[0];
  if (sentence) return sentence.trim();
  if (which === "first") return plain;
  // Live, a sentence still streaming shows its whole words so far, never half a word.
  const whole = /\s$/.test(text) ? plain : plain.replace(/\s*\S+$/, "");
  return whole ? `${whole}…` : "";
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
export function turnActivity(sequence: readonly SequenceEntry[]): Activity<LeadStep, SequenceEntry>[] {
  return groupActivity<LeadStep, SequenceEntry>(sequence, (entry) => {
    switch (entry.kind) {
      case "thinking": {
        if (entry.live) return { type: "skip" };
        const thought = thoughtOf(entry.segment);
        return thought ? { type: "thought", thought } : { type: "skip" };
      }
      case "orchestrator": {
        const [step] = entry.steps;
        if (!step || entry.steps.length > 1) return { type: "break" };
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
export function workerActivity(entries: readonly ThreadEntry[], live: boolean): Activity<ActionItem, ThreadEntry>[] {
  type Flat = ThreadEntry | { kind: "action"; item: ActionItem };
  const flat = entries.flatMap((entry): Flat[] =>
    entry.kind === "actions" ? entry.items.map((item) => ({ kind: "action" as const, item })) : [entry],
  );
  const last = flat.at(-1);
  return groupActivity(flat, (entry) => {
    if (entry.kind === "action") {
      return workerPlumbing(entry.item) ? { type: "skip" } : { type: "step", key: entry.item.key, step: entry.item };
    }
    // An answered approval leaves no trace: the action's own row tells what came of it.
    if (entry.kind === "item" && entry.item.kind === "approval" && entry.item.resolution) return { type: "skip" };
    if (entry.kind === "item" && entry.item.kind === "reasoning") {
      const { item } = entry;
      // The thought it is thinking now is the live line's.
      if (live && item.streaming && entry === last) return { type: "skip" };
      if (!item.text.trim()) return { type: "skip" };
      return { type: "thought", thought: { key: item.key, text: item.text, ms: Math.max(0, item.endedAtMs - item.startedAtMs) } };
    }
    return { type: "break" };
  }) as Activity<ActionItem, ThreadEntry>[]; // An action is always a step or skipped, never an entry.
}
