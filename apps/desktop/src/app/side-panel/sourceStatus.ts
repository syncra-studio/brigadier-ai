import type { SourceState, SourceStatus } from "@/ipc/generated";

/** How a changed file differs, as Source and the tree mark it: a letter in its colour. */
export const LETTERS: Record<SourceStatus, { letter: string; label: string; className: string }> = {
  modified: { letter: "M", label: "Modified", className: "text-warning" },
  added: { letter: "A", label: "Added", className: "text-success" },
  deleted: { letter: "D", label: "Deleted", className: "text-destructive" },
  renamed: { letter: "R", label: "Renamed", className: "text-link" },
  copied: { letter: "C", label: "Copied", className: "text-link" },
  typeChanged: { letter: "T", label: "Type changed", className: "text-warning" },
  untracked: { letter: "U", label: "Untracked", className: "text-success" },
  conflicted: { letter: "!", label: "Conflicted", className: "text-destructive" },
};

/** Which change a folder holding several shows: the one most in need of a look. */
const SEVERITY: Record<SourceStatus, number> = {
  conflicted: 5, deleted: 4, modified: 3, typeChanged: 3, renamed: 2, copied: 2, added: 1, untracked: 1,
};

export type ChangeMarks = {
  /** Each changed file's status: its unstaged change first, else its staged one. */
  files: ReadonlyMap<string, SourceStatus>;
  /** Each folder holding changes: the most severe of them. */
  folders: ReadonlyMap<string, SourceStatus>;
};

/** What the tree marks, from the checkout's source state. */
export function changeMarks(state: SourceState | null | undefined): ChangeMarks {
  const files = new Map<string, SourceStatus>();
  for (const file of state?.staged ?? []) files.set(file.path, file.status);
  for (const file of state?.changes ?? []) files.set(file.path, file.status);
  const folders = new Map<string, SourceStatus>();
  for (const [path, status] of files) {
    for (let slash = path.lastIndexOf("/"); slash > 0; slash = path.lastIndexOf("/", slash - 1)) {
      const folder = path.slice(0, slash);
      const held = folders.get(folder);
      if (held && SEVERITY[held] >= SEVERITY[status]) continue;
      folders.set(folder, status);
    }
  }
  return { files, folders };
}
