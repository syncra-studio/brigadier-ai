import type { CleanCategory, CleanFailure, CleanItem, CleanReport, StorageReport } from "@/ipc/generated";
import { formatBytes } from "@/lib/format";

/*
 * Free up space, without the screen: how a scan's items fall into the plain groups, what the
 * one button removes, what is kept on purpose and why, and the words of the confirmation and
 * the result.
 */

/** The groups the sweep shows, in order. */
export const SWEEP_GROUPS: readonly { category: CleanCategory; title: string }[] = [
  { category: "finishedWork", title: "Finished sessions’ work folders" },
  { category: "buildFiles", title: "Build files in idle sessions" },
  { category: "agentFiles", title: "Agent session files" },
  { category: "temporary", title: "Temporary files" },
  { category: "oldLogs", title: "Old logs" },
  { category: "deletedLeftovers", title: "Deleted conversations’ leftovers" },
];

/** Every group, for choosing items by hand (Advanced). */
export const ALL_GROUPS: readonly { category: CleanCategory; title: string }[] = [
  ...SWEEP_GROUPS,
  { category: "brains", title: "Brains of removed projects" },
  { category: "models", title: "Downloaded models" },
  { category: "recordings", title: "Old recordings" },
  { category: "processes", title: "Other Brigadier daemons" },
];

const SWEEP = new Set(SWEEP_GROUPS.map((group) => group.category));

export type Group = { category: CleanCategory; title: string; bytes: number; items: CleanItem[] };

/** Something kept on purpose: an item, or a counted line. */
export type Kept = { key: string; label: string; bytes: number; reason: string; path: string | null };

export type Sweep = {
  /** The groups with something to remove, each with what it removes. */
  groups: Group[];
  bytes: number;
  ids: string[];
  /** What is kept on purpose, with why. */
  kept: Kept[];
  keptBytes: number;
};

const sum = (items: readonly { bytes: number }[]) => items.reduce((total, item) => total + item.bytes, 0);

/** Items of a group in a fixed order of groups (`order`), empty groups left out. */
export function grouped(
  items: readonly CleanItem[],
  order: readonly { category: CleanCategory; title: string }[] = ALL_GROUPS,
): Group[] {
  return order
    .map(({ category, title }) => {
      const inGroup = items.filter((item) => item.category === category);
      return { category, title, bytes: sum(inGroup), items: inGroup };
    })
    .filter((group) => group.items.length > 0);
}

/** What the one button removes (the safe items), and what stays on purpose. */
export function sweep(report: StorageReport): Sweep {
  const swept = report.items.filter((item) => item.checked && item.selectable && SWEEP.has(item.category));
  const groups = grouped(swept, SWEEP_GROUPS);
  const kept: Kept[] = [
    ...report.items
      .filter((item) => SWEEP.has(item.category) && !(item.checked && item.selectable))
      .map((item) => ({ key: item.id, label: item.label, bytes: item.bytes, reason: item.reason, path: item.path })),
    ...report.kept.map((line, index) => ({
      key: `kept-${index}`,
      label: line.label,
      bytes: line.bytes,
      reason: line.reason,
      path: null,
    })),
  ];
  return { groups, bytes: sum(swept), ids: swept.map((item) => item.id), kept, keptBytes: sum(kept) };
}

/** The picked items of a scan, as the confirmation and the button show them. */
export function picked(report: StorageReport, ids: ReadonlySet<string>): Sweep {
  const items = report.items.filter((item) => ids.has(item.id) && item.selectable);
  const all = sweep(report);
  return { groups: grouped(items), bytes: sum(items), ids: items.map((item) => item.id), kept: all.kept, keptBytes: all.keptBytes };
}

export function count(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

/** The line under the page's heading. */
export function summary(report: StorageReport, plan: Sweep): string {
  const uses = `Brigadier uses ${formatBytes(report.totalBytes)} on this computer.`;
  if (plan.ids.length === 0) return `${uses} Nothing to free up: Brigadier is tidy.`;
  if (plan.bytes === 0) return `${uses} A few small leftovers can be tidied up.`;
  return `${uses} ${formatBytes(plan.bytes)} can be freed.`;
}

/** The one button's words (and the confirmation's). */
export function action(plan: Sweep): string {
  if (plan.ids.length === 0) return "Nothing to free up";
  return plan.bytes > 0 ? `Free up ${formatBytes(plan.bytes)}` : `Tidy up ${count(plan.ids.length, "item", "items")}`;
}

/** The confirmation's words: what goes, group by group, and what stays. */
export function confirmation(plan: Sweep): { title: string; lines: string[]; footer: string } {
  const lines = plan.groups.map(
    (group) => `${group.title}: ${count(group.items.length, "item", "items")}, ${formatBytes(group.bytes)}`,
  );
  const trash = plan.groups.flatMap((group) => group.items).filter((item) => item.toTrash);
  const kept =
    plan.kept.length > 0
      ? `${count(plan.kept.length, "thing is", "things are")} kept on purpose. Nothing else is touched.`
      : "Nothing else is touched.";
  const footer = trash.length > 0 ? `${count(trash.length, "item goes", "items go")} to the Trash. ${kept}` : kept;
  return { title: `${action(plan)}?`, lines, footer };
}

/** The result's words. */
export function result(cleaned: CleanReport): { title: string; lines: string[] } {
  const freed = cleaned.reclaimedBytes;
  const title =
    freed > 0 || cleaned.removed > 0
      ? `Freed ${formatBytes(freed)}.`
      : cleaned.failures.length > 0
        ? "Nothing could be removed."
        : "Nothing was removed.";
  const lines: string[] = [];
  if (cleaned.trashedBytes > 0) {
    lines.push(`${formatBytes(cleaned.trashedBytes)} moved to the Trash: empty it to get that space back.`);
  }
  if (cleaned.codexThreadsDeleted > 0) {
    lines.push(
      `Deleted ${count(cleaned.codexThreadsDeleted, "Codex thread", "Codex threads")}. The Codex app keeps its own list, so it may go on showing ${cleaned.codexThreadsDeleted === 1 ? "it" : "them"}.`,
    );
  }
  if (cleaned.failures.length > 0) {
    lines.push(`${count(cleaned.failures.length, "item stays", "items stay")}, as below.`);
  }
  return { title, lines };
}

/** Why an item stayed, as a plain sentence; the daemon's own words are the detail under it. */
export function failure(failed: CleanFailure): { plain: string; detail: string } {
  const error = failed.error.toLowerCase();
  const why =
    error.includes("running") || error.includes("runs in it") || error.includes("uses it now") || error.includes("in use")
      ? "something is using it now"
      : error.includes("changed since") || error.includes("not what was shown") || error.includes("is back") || error.includes("again")
        ? "it changed since the scan"
        : error.includes("unsaved changes") || error.includes("no branch")
          ? "it has work that isn't on a branch"
          : "Brigadier couldn't remove it safely";
  const detail = failed.error.charAt(0).toUpperCase() + failed.error.slice(1);
  return { plain: `Left in place: ${why}.`, detail };
}
