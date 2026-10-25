import type { PluginItemView, PluginKind, PluginsView } from "./types";

/*
 * What the Plugins page shows, from the daemon's view: each tab's items, each card's line and
 * tag, in plain words.
 */

export const KIND_LABEL: Record<PluginKind, string> = { server: "MCP server", skill: "Skill" };
export const TAB_LABEL: Record<PluginKind, string> = { server: "MCP servers", skill: "Skills" };

export type Tone = "muted" | "warning" | "error";
export type Line = { text: string; tone: Tone };

/** "a", "a and b", "a, b and c". */
export function listNames(names: string[]): string {
  if (names.length <= 1) return names[0] ?? "";
  return `${names.slice(0, -1).join(", ")} and ${names.at(-1)}`;
}

/** A folder's last part: a folder that isn't a project goes by it. */
export const folderName = (path: string) => path.split("/").filter(Boolean).pop() ?? path;

/** The names of the places an item is kept to: its projects', then its other folders'. */
export function placeNames(view: PluginsView, item: PluginItemView): string[] {
  if (item.scope.type === "allProjects") return [];
  return [
    ...item.scope.projectIds.map((id) => view.projects.find((project) => project.id === id)?.name ?? id),
    ...item.scope.folders.map(folderName),
  ];
}

/** A card's line: its problem, else why it can't be changed, else what it does, else how it runs. */
export function lineOf(item: PluginItemView): Line {
  if (item.problem) return { text: item.problem.message, tone: "warning" };
  if (item.locked) return { text: item.locked, tone: "muted" };
  return { text: item.description || item.detail, tone: "muted" };
}

/** Where an item is on, as its tag: "Global", a project's name, "3 projects"; null when it's in none. */
export function chipOf(view: PluginsView, item: PluginItemView): string | null {
  if (item.scope.type === "allProjects") return "Global";
  const names = placeNames(view, item);
  if (names.length === 0) return null;
  return names.length === 1 ? names[0]! : `${names.length} projects`;
}

function matches(item: PluginItemView, query: string): boolean {
  const words = query.trim().toLowerCase();
  return !words || `${item.name} ${item.description} ${item.detail}`.toLowerCase().includes(words);
}

/** Tab `kind`'s items that match `query`, in the daemon's order: the ones with a problem first, then A–Z. */
export const listed = (view: PluginsView, kind: PluginKind, query: string): PluginItemView[] =>
  view.items.filter((item) => item.kind === kind && matches(item, query));

/** An address without its https:// (ws:// and the like say something, so they stay). */
export const shownDetail = (detail: string) => detail.replace(/^https:\/\//, "");

/** What a won't-start message says is wrong: "unknown type "websocket"". */
export const wontStartReason = (message: string) => message.replace(/^Won't start:\s*/i, "").replace(/\.$/, "");
