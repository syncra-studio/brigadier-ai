import type { Conversation, Project } from "@/ipc/generated";

/*
 * The Archived page without the screen: which archived conversations show for a search,
 * a kind, a project and an order, in what groups, and what its menus offer.
 */

export type ArchivedKind = "all" | "session" | "chat";
export type ArchivedSort = "updated" | "created" | "alphabetical";
/** A project's id, every project, or the chats outside any project. */
export type ArchivedProject = "all" | "chats" | (string & {});

export type ArchivedFilter = {
  query: string;
  kind: ArchivedKind;
  sort: ArchivedSort;
  project: ArchivedProject;
};

export const ALL_ARCHIVED: ArchivedFilter = { query: "", kind: "all", sort: "updated", project: "all" };

export type ArchivedGroup = { key: string; label: string; conversations: Conversation[] };

/** The key of a conversation's group: its project, or the chats. */
function groupKey(conversation: Conversation): string {
  return conversation.kind === "session" && conversation.projectId ? conversation.projectId : "chats";
}

function compare(sort: ArchivedSort): (a: Conversation, b: Conversation) => number {
  switch (sort) {
    case "created":
      return (a, b) => b.createdAtMs - a.createdAtMs;
    case "alphabetical":
      return (a, b) => a.title.localeCompare(b.title, undefined, { sensitivity: "base", numeric: true });
    case "updated":
      return (a, b) => b.updatedAtMs - a.updatedAtMs;
  }
}

/** Every archived conversation, newest first. */
export function archivedOf(conversations: Record<string, Conversation>): Conversation[] {
  return Object.values(conversations)
    .filter((conversation) => conversation.lifecycle === "archived")
    .toSorted(compare("updated"));
}

/** Whether `title` holds every word of `query`, in any case. */
function matches(title: string, query: string): boolean {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  const haystack = title.toLowerCase();
  return words.every((word) => haystack.includes(word));
}

/**
 * The archived conversations a filter shows, in groups: a group per project, in the order of
 * its first conversation, then the chats. Within a group, the filter's order.
 */
export function archivedGroups(
  archived: readonly Conversation[],
  projects: Record<string, Project>,
  filter: ArchivedFilter,
): ArchivedGroup[] {
  const shown = archived
    .filter((conversation) => filter.kind === "all" || conversation.kind === filter.kind)
    .filter((conversation) => filter.project === "all" || groupKey(conversation) === filter.project)
    .filter((conversation) => matches(conversation.title, filter.query))
    .toSorted(compare(filter.sort));
  const groups = new Map<string, ArchivedGroup>();
  for (const conversation of shown) {
    const key = groupKey(conversation);
    let group = groups.get(key);
    if (!group) {
      group = { key, label: key === "chats" ? "Chats" : (projects[key]?.name ?? "Project"), conversations: [] };
      groups.set(key, group);
    }
    group.conversations.push(conversation);
  }
  return [...groups.values()].toSorted((a, b) => Number(a.key === "chats") - Number(b.key === "chats"));
}

/** The rows as they show, top to bottom (for picking a range). */
export function shownIds(groups: readonly ArchivedGroup[]): string[] {
  return groups.flatMap((group) => group.conversations.map((conversation) => conversation.id));
}

/** The project menu's choices: every project with something archived, by name, then the chats. */
export function projectChoices(
  archived: readonly Conversation[],
  projects: Record<string, Project>,
): { value: ArchivedProject; label: string }[] {
  const keys = new Set(archived.map(groupKey));
  const named = [...keys]
    .filter((key) => key !== "chats")
    .map((key) => ({ value: key, label: projects[key]?.name ?? "Project" }))
    .toSorted((a, b) => a.label.localeCompare(b.label, undefined, { sensitivity: "base" }));
  return [
    { value: "all", label: "All projects" },
    ...named,
    ...(keys.has("chats") ? [{ value: "chats", label: "Chats" }] : []),
  ];
}

export const KIND_CHOICES: readonly { value: ArchivedKind; label: string }[] = [
  { value: "all", label: "All chats" },
  { value: "session", label: "Sessions" },
  { value: "chat", label: "Chats" },
];

export const SORT_CHOICES: readonly { value: ArchivedSort; label: string }[] = [
  { value: "updated", label: "Updated" },
  { value: "created", label: "Created" },
  { value: "alphabetical", label: "Alphabetical" },
];

/** "1 chat", "3 sessions": a group's count, by what it holds. */
export function groupCount(group: ArchivedGroup): string {
  const count = group.conversations.length;
  const noun = group.key === "chats" ? "chat" : "session";
  return `${count} ${noun}${count === 1 ? "" : "s"}`;
}

/** Whether the filter narrows the list (so an empty list means "nothing matches"). */
export function narrowed(filter: ArchivedFilter): boolean {
  return filter.query.trim() !== "" || filter.kind !== "all" || filter.project !== "all";
}
