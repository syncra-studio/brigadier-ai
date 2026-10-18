import type {
  CleanCategory,
  CleanItem,
  CleanReport,
  ProjectUsage,
  SharedPart,
  StorageReport,
} from "@/ipc/generated";
import { formatAgo } from "@/lib/format";

/*
 * Settings › Storage without the screen, drawn to the user's design: what the disk use is
 * made of, each project's share, the groups Free up space reviews (ticked or not, and what is
 * kept on purpose), and the words of each state.
 */

const KB = 1024;
const MB = 1024 * KB;
const GB = 1024 * MB;
const DAY = 24 * 60 * 60 * 1000;

/** A project not used for this long counts as not in use any more. */
export const UNUSED_AFTER_MS = 90 * DAY;
/** Projects shown before "Show all", and per page once it is open. */
export const TOP_PROJECTS = 4;
export const PROJECTS_PAGE = 20;
/** Items a group shows before "Show more". */
export const GROUP_ITEMS = 4;

/** A size as the design writes it: "0 B", "444 KB", "5.1 MB", "1.2 GB". */
export function size(bytes: number): string {
  if (bytes < KB) return `${bytes} B`;
  if (bytes < MB) return `${Math.round(bytes / KB)} KB`;
  if (bytes < GB) return `${(bytes / MB).toFixed(1)} MB`;
  return `${(bytes / GB).toFixed(1)} GB`;
}

export function count(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

/** One part of a bar: what it is, its size and its colour (a `bg-*` class). */
export type Part = { key: string; label: string; bytes: number; color: string };

export const PART_COLOR = {
  work: "bg-storage-work",
  conversations: "bg-storage-conversations",
  brains: "bg-storage-brains",
  models: "bg-storage-models",
  logs: "bg-settings-success",
  other: "bg-storage-other",
} as const;

const shared = (report: StorageReport, part: SharedPart) =>
  report.shared.find((entry) => entry.part === part)?.bytes ?? 0;

const sum = (items: readonly { bytes: number }[]) => items.reduce((total, item) => total + item.bytes, 0);

/** A project's disk use in all. */
export function projectBytes(project: ProjectUsage): number {
  return project.worktreesBytes + project.brainBytes + project.blobsBytes + project.scratchBytes;
}

/**
 * What Brigadier's disk use is made of, for the bar and its legend, in the design's order. The
 * parts add up to the scan's total: what no other part counts is Other.
 */
export function usageParts(report: StorageReport): Part[] {
  const projects = (pick: (project: ProjectUsage) => number) =>
    report.projects.reduce((total, project) => total + pick(project), 0);
  const parts: Part[] = [
    { key: "work", label: "Work folders", bytes: projects((p) => p.worktreesBytes), color: PART_COLOR.work },
    {
      key: "conversations",
      label: "Conversations",
      bytes: projects((p) => p.blobsBytes) + shared(report, "otherBlobs") + shared(report, "database"),
      color: PART_COLOR.conversations,
    },
    {
      key: "brains",
      label: "Brains",
      bytes: projects((p) => p.brainBytes) + shared(report, "personalBrain"),
      color: PART_COLOR.brains,
    },
    { key: "models", label: "Models", bytes: shared(report, "models"), color: PART_COLOR.models },
    {
      key: "logs",
      label: "Logs & recordings",
      bytes: shared(report, "logs") + shared(report, "recordings"),
      color: PART_COLOR.logs,
    },
  ];
  const other = Math.max(
    0,
    report.totalBytes - sum(parts),
    projects((p) => p.scratchBytes) + shared(report, "other"),
  );
  return [...parts, { key: "other", label: "Other", bytes: other, color: PART_COLOR.other }];
}

/** A project's share bar and its opened rows: Brains, conversations, work folders. */
export function projectParts(project: ProjectUsage): Part[] {
  return [
    { key: "brains", label: "Brains", bytes: project.brainBytes, color: PART_COLOR.brains },
    { key: "conversations", label: "Conversations", bytes: project.blobsBytes, color: PART_COLOR.conversations },
    {
      key: "work",
      label: "Work folders",
      bytes: project.worktreesBytes + project.scratchBytes,
      color: PART_COLOR.work,
    },
  ];
}

/** What every project shares: models, logs and recordings, and the rest no project holds. */
export function sharedParts(report: StorageReport): Part[] {
  const models = shared(report, "models");
  const logs = shared(report, "logs") + shared(report, "recordings");
  const projects = report.projects.reduce((total, project) => total + projectBytes(project), 0);
  return [
    { key: "models", label: "Models", bytes: models, color: PART_COLOR.models },
    { key: "logs", label: "Logs & recordings", bytes: logs, color: PART_COLOR.logs },
    {
      key: "other",
      label: "Other",
      bytes: Math.max(0, report.totalBytes - projects - models - logs),
      color: PART_COLOR.other,
    },
  ];
}

/** "Used today", "Used yesterday", "Used 4 days ago", "Used 2 weeks ago", "Used 5 months ago". */
const startOf = (ms: number) => new Date(ms).setHours(0, 0, 0, 0);

export function usedWords(lastUsedMs: number, nowMs: number): string {
  const days = Math.max(0, Math.round((startOf(nowMs) - startOf(lastUsedMs)) / DAY));
  if (days === 0) return "Used today";
  if (days === 1) return "Used yesterday";
  if (days < 7) return `Used ${days} days ago`;
  if (days < 30) return `Used ${count(Math.floor(days / 7), "week", "weeks")} ago`;
  if (days < 365) return `Used ${count(Math.floor(days / 30), "month", "months")} ago`;
  return `Used ${count(Math.floor(days / 365), "year", "years")} ago`;
}

/** "Checked just now", "Checked 3m ago". */
export function checkedWords(checkedAtMs: number, nowMs: number): string {
  return nowMs - checkedAtMs < 60_000 ? "Checked just now" : `Checked ${formatAgo(checkedAtMs, nowMs)}`;
}

export function isUnused(project: ProjectUsage, nowMs: number): boolean {
  return nowMs - project.lastUsedMs > UNUSED_AFTER_MS;
}

export type ProjectSort = "largest" | "recent" | "name";

export const PROJECT_SORTS: readonly { value: ProjectSort; label: string }[] = [
  { value: "largest", label: "Largest first" },
  { value: "recent", label: "Recently used" },
  { value: "name", label: "Name" },
];

const byName = (a: ProjectUsage, b: ProjectUsage) => a.name.localeCompare(b.name);

/** The projects in the order picked, filtered by the search. */
export function listProjects(projects: readonly ProjectUsage[], sort: ProjectSort, query = ""): ProjectUsage[] {
  const wanted = query.trim().toLowerCase();
  const found = wanted
    ? projects.filter(
        (project) =>
          project.name.toLowerCase().includes(wanted) || (project.path ?? "").toLowerCase().includes(wanted),
      )
    : projects;
  return found.toSorted((a, b) =>
    sort === "largest"
      ? projectBytes(b) - projectBytes(a) || byName(a, b)
      : sort === "recent"
        ? b.lastUsedMs - a.lastUsedMs || byName(a, b)
        : byName(a, b),
  );
}

/** The projects not in use, and what their Brains and conversations take. */
export function unusedProjects(report: StorageReport, nowMs: number): { projects: ProjectUsage[]; bytes: number } {
  // One freed up already has nothing left to free.
  const projects = report.projects.filter(
    (project) => isUnused(project, nowMs) && project.conversations + project.brainBytes + project.blobsBytes > 0,
  );
  return { projects, bytes: projects.reduce((total, p) => total + p.brainBytes + p.blobsBytes, 0) };
}

/** The groups Free up space reviews, by the design's names. */
const GROUP_TITLES: Record<CleanCategory, string> = {
  temporary: "Temporary files",
  agentFiles: "Agent session files",
  finishedWork: "Finished sessions' work folders",
  buildFiles: "Build files in idle sessions",
  oldLogs: "Old logs",
  deletedLeftovers: "Deleted conversations' leftovers",
  brains: "Brains of removed projects",
  models: "Downloaded models",
  recordings: "Old recordings",
  processes: "Other Brigadier daemons",
};

/** How the summary names what can go, lower-case after the first. */
const GROUP_PHRASES: Record<CleanCategory, string> = {
  temporary: "temporary files",
  agentFiles: "files left by sessions that are gone",
  finishedWork: "finished sessions' work folders",
  buildFiles: "build files in idle sessions",
  oldLogs: "old logs",
  deletedLeftovers: "what deleted conversations left",
  brains: "Brains of removed projects",
  models: "downloaded models",
  recordings: "old recordings",
  processes: "other Brigadier daemons",
};

export type Group = { category: CleanCategory; title: string; items: CleanItem[]; bytes: number };

/** Something kept on purpose: an item that can't be picked, or a counted line. */
export type Kept = { key: string; label: string; bytes: number; reason: string; path: string | null };

/** What can be removed, in groups (largest first), and what is kept on purpose. */
export function review(report: StorageReport): { groups: Group[]; kept: Kept[]; keptBytes: number } {
  const byCategory = new Map<CleanCategory, CleanItem[]>();
  for (const item of report.items) {
    if (!item.selectable) continue;
    byCategory.set(item.category, [...(byCategory.get(item.category) ?? []), item]);
  }
  const groups = [...byCategory]
    .map(([category, items]) => ({ category, title: GROUP_TITLES[category], items, bytes: sum(items) }))
    .toSorted((a, b) => b.bytes - a.bytes);
  const kept: Kept[] = [
    ...report.items
      .filter((item) => !item.selectable)
      .map((item) => ({ key: item.id, label: item.label, bytes: item.bytes, reason: item.reason, path: item.path })),
    ...report.kept.map((line, index) => ({
      key: `kept-${index}`,
      label: line.label,
      bytes: line.bytes,
      reason: line.reason,
      path: null,
    })),
  ];
  return { groups, kept, keptBytes: sum(kept) };
}

/** What starts ticked: everything safe to remove (the daemon's sweep). */
export function startTicked(report: StorageReport, projectId?: string): Set<string> {
  return new Set(
    report.items
      .filter((item) => item.checked && item.selectable && (!projectId || item.projectId === projectId))
      .map((item) => item.id),
  );
}

/** What can go: the ticked-to-start items' size. */
export function canGoBytes(report: StorageReport, projectId?: string): number {
  const ticked = startTicked(report, projectId);
  return sum(report.items.filter((item) => ticked.has(item.id)));
}

/** The summary's line at rest: what can go, in plain words. */
export function summaryLine(report: StorageReport): string {
  const ticked = startTicked(report);
  const phrases = review(report)
    .groups.filter((group) => group.items.some((item) => ticked.has(item.id) && item.bytes > 0))
    .map((group) => GROUP_PHRASES[group.category]);
  if (phrases.length === 0) {
    return ticked.size > 0 ? "A few small leftovers. Nothing goes until you confirm." : "Brigadier is tidy.";
  }
  const first = phrases[0]!.charAt(0).toUpperCase() + phrases[0]!.slice(1);
  const rest = phrases.slice(1);
  const listed =
    rest.length === 0 ? first : `${[first, ...rest.slice(0, -1)].join(", ")}, and ${rest[rest.length - 1]}`;
  return `${listed}. Nothing goes until you confirm.`;
}

/** How a group's box stands. */
export function tickState(group: Group, ticked: ReadonlySet<string>): "all" | "some" | "none" {
  const on = group.items.filter((item) => ticked.has(item.id)).length;
  return on === 0 ? "none" : on === group.items.length ? "all" : "some";
}

/** What is picked: its size, how many, of how many. */
export function selection(report: StorageReport, ticked: ReadonlySet<string>): { bytes: number; picked: number; of: number } {
  const items = report.items.filter((item) => item.selectable);
  const picked = items.filter((item) => ticked.has(item.id));
  return { bytes: sum(picked), picked: picked.length, of: items.length };
}

/** What a clean took and what stayed (with why), by the scan's items. */
export function outcome(
  report: StorageReport,
  ids: readonly string[],
  cleaned: CleanReport,
): { removed: CleanItem[]; stayed: { item: CleanItem | null; label: string; error: string }[] } {
  const failed = new Set(cleaned.failures.map((failure) => failure.id));
  const asked = new Set(ids);
  return {
    removed: report.items.filter((item) => asked.has(item.id) && item.selectable && !failed.has(item.id)),
    stayed: cleaned.failures.map((failure) => ({
      item: report.items.find((item) => item.id === failure.id) ?? null,
      label: failure.label,
      error: failure.error.charAt(0).toUpperCase() + failure.error.slice(1),
    })),
  };
}

/** The done line: what went, and what Brigadier uses now. */
export function doneLine(cleaned: CleanReport, totalBytes: number | null): string {
  const parts = [`${count(cleaned.removed, "item", "items")} removed just now`];
  if (cleaned.failures.length > 0) parts.push(`${count(cleaned.failures.length, "item", "items")} stayed`);
  if (cleaned.trashedBytes > 0) parts.push(`${size(cleaned.trashedBytes)} in the Trash`);
  if (totalBytes !== null) parts.push(`${size(totalBytes)} now used by Brigadier`);
  return parts.join(" · ");
}

/** "142 chats and sessions, 6 of them archived". */
export function conversationsLine(project: ProjectUsage): string {
  const all = project.conversations === 1 ? "1 chat or session" : `${project.conversations} chats and sessions`;
  if (project.archived === 0) return all;
  if (project.conversations === 1) return `${all}, archived`;
  return `${all}, ${project.archived === project.conversations ? "all" : project.archived} of them archived`;
}
