import { fuzzyMatch } from "@/components/assistant-ui/elements/menuMatch";
import type { Mention } from "@/ipc/generated";

/** A worker the composer can @-mention, `title` its name as the user reads it. */
export type MentionTarget = { id: string; number: number; title: string; state: string };

/** One row the `@` menu can show: the mention it makes and how the row reads. */
export type MentionRow = {
  mention: Mention;
  /** What the mention leaves in the text, after `@`. */
  label: string;
  /** The row's name (a file's name for its path, a worker's title). */
  name: string;
  /** Grey text after the name (a file's folder). */
  detail?: string | undefined;
  /** Grey text at the row's end (a worker's state). */
  trailing?: string | undefined;
};

/** A part of the `@` menu: its heading (none for the ranked results), rows and a grey note. */
export type MentionSection = { title: string | null; rows: MentionRow[]; note?: string | undefined };

/** What can be mentioned: the open session's workers, its checkout's files and folders, other chats. */
export type MentionSources = {
  workers: readonly MentionTarget[];
  files: readonly string[];
  folders: readonly string[];
  /** Newest first. */
  chats: readonly { id: string; title: string }[];
};

/** Rows of results for a query; the menu shows no more at once. */
export const RESULT_ROWS = 8;
/** Folders and chats listed under their heading before anything is typed. */
const SECTION_ROWS = 5;

/** Every folder holding one of `files`, by its path from the checkout's root. */
export function foldersOf(files: readonly string[]): string[] {
  const folders = new Set<string>();
  for (const file of files) {
    for (let slash = file.indexOf("/"); slash > 0; slash = file.indexOf("/", slash + 1)) {
      folders.add(file.slice(0, slash));
    }
  }
  return [...folders].toSorted();
}

function pathRow(type: "file" | "folder", path: string): MentionRow {
  const slash = path.lastIndexOf("/");
  return {
    mention: { type, path },
    label: path,
    name: path.slice(slash + 1),
    detail: slash > 0 ? path.slice(0, slash) : undefined,
  };
}

function workerRow(target: MentionTarget): MentionRow {
  // By its title, as everywhere the user reads it; `task-N` is what it leaves in the text.
  return { mention: { type: "task", id: target.id }, label: `task-${target.number}`, name: target.title, trailing: target.state };
}

function chatRow(chat: { id: string; title: string }): MentionRow {
  return { mention: { type: "chat", id: chat.id, title: chat.title }, label: chat.title, name: chat.title };
}

/**
 * How well `query` matches a row (lower is better), or null: its name matched at the start, then
 * anywhere, then letter by letter; a file or folder also by its path, after its name.
 */
function score(row: MentionRow, query: string): number | null {
  const name = Math.min(
    fuzzyMatch(row.name, query)?.rank ?? 9,
    row.mention.type === "task" ? (fuzzyMatch(row.label, query)?.rank ?? 9) : 9,
  );
  if (name < 9) return name * 2;
  if (row.mention.type !== "file" && row.mention.type !== "folder") return null;
  const path = fuzzyMatch(row.label, query);
  if (!path) return null;
  return path.rank === 2 ? 5 : 3;
}

const KIND_ORDER: Record<Mention["type"], number> = { task: 0, file: 1, folder: 2, chat: 3, page: 4 };

/**
 * The `@` menu for `query`. Before anything is typed, a section per kind under its heading:
 * the workers, Files (only a note: there are too many to list), the top folders and the
 * newest chats. With a query, one list of the best matches across every kind, at most
 * `RESULT_ROWS`.
 */
export function mentionSections(query: string, sources: MentionSources): MentionSection[] {
  if (!query) {
    const sections: MentionSection[] = [];
    if (sources.workers.length > 0) sections.push({ title: "Workers", rows: sources.workers.map(workerRow) });
    if (sources.files.length > 0) sections.push({ title: "Files", rows: [], note: "Type to search for files" });
    const top = sources.folders.filter((folder) => !folder.includes("/")).slice(0, SECTION_ROWS);
    if (top.length > 0) sections.push({ title: "Folders", rows: top.map((folder) => pathRow("folder", folder)) });
    if (sources.chats.length > 0) sections.push({ title: "Chats", rows: sources.chats.slice(0, SECTION_ROWS).map(chatRow) });
    return sections;
  }
  const rows = [
    ...sources.workers.map(workerRow),
    ...sources.files.map((file) => pathRow("file", file)),
    ...sources.folders.map((folder) => pathRow("folder", folder)),
    ...sources.chats.map(chatRow),
  ];
  const ranked = rows
    .map((row) => ({ row, score: score(row, query) }))
    .filter((entry): entry is { row: MentionRow; score: number } => entry.score !== null)
    .toSorted(
      (a, b) =>
        a.score - b.score ||
        a.row.name.length - b.row.name.length ||
        a.row.label.length - b.row.label.length ||
        KIND_ORDER[a.row.mention.type] - KIND_ORDER[b.row.mention.type],
    )
    .slice(0, RESULT_ROWS)
    .map((entry) => entry.row);
  return [{ title: null, rows: ranked }];
}
