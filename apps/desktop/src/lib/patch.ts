/**
 * Reads git's unified text patch into rows the Review tab draws: each file's lines with
 * their numbers, and the stretches of unchanged lines folded into "N unmodified lines".
 */

export type DiffLine = {
  kind: "context" | "add" | "del";
  /** Its number in the old file (absent for an added line). */
  old: number | null;
  /** Its number in the new file (absent for a deleted line). */
  new: number | null;
  text: string;
};

export type DiffRow =
  | DiffLine
  /** Unchanged lines left out: `lines` when the patch carries them, else only how many. */
  | { kind: "gap"; id: number; count: number; lines: DiffLine[] | null };

export type PatchFile = {
  /** The new path (the old one for a deleted file). */
  path: string;
  binary: boolean;
  rows: DiffRow[];
};

/** Unchanged lines kept in view on each side of a change, for context around the diff. */
const KEEP = 1;

/** What git writes after `\` in a quoted path, other than octal bytes. */
const ESCAPES: Readonly<Record<string, number>> = {
  a: 7,
  b: 8,
  t: 9,
  n: 10,
  v: 11,
  f: 12,
  r: 13,
  '"': 34,
  "\\": 92,
};

/**
 * Undoes git's C-style quoting of a path with unusual characters: escapes, and a non-ASCII
 * name's UTF-8 bytes in octal (`"caf\303\251.md"` is `café.md`).
 */
function unquote(path: string): string {
  if (path.length < 2 || !path.startsWith('"') || !path.endsWith('"')) return path;
  const chars = Array.from(path.slice(1, -1));
  const encoder = new TextEncoder();
  const bytes: number[] = [];
  for (let at = 0; at < chars.length; at++) {
    const char = chars[at] as string;
    const next = chars[at + 1] ?? "";
    if (char !== "\\") {
      bytes.push(...encoder.encode(char));
    } else if (/[0-7]/.test(next)) {
      const octal = /^[0-7]{1,3}/u.exec(chars.slice(at + 1, at + 4).join(""))?.[0] ?? "";
      bytes.push(Number.parseInt(octal, 8));
      at += octal.length;
    } else {
      bytes.push(ESCAPES[next] ?? next.charCodeAt(0));
      at += 1;
    }
  }
  return new TextDecoder().decode(new Uint8Array(bytes));
}

function stripPrefix(path: string): string {
  const unquoted = unquote(path.trim());
  return unquoted.replace(/^[ab]\//, "");
}

/** The lines of each file of `patch`, keyed by path. */
function readFiles(patch: string): Map<string, { binary: boolean; hunks: Hunk[] }> {
  const files = new Map<string, { binary: boolean; hunks: Hunk[] }>();
  let current: { binary: boolean; hunks: Hunk[] } | null = null;
  let oldPath: string | null = null;
  let hunk: Hunk | null = null;
  let oldLine = 0;
  let newLine = 0;
  const lines = patch.split("\n");
  if (lines.at(-1) === "") lines.pop();
  for (const line of lines) {
    if (line.startsWith("diff --git ")) {
      current = { binary: false, hunks: [] };
      hunk = null;
      oldPath = null;
      // A file without hunks (binary, mode only) is named here; `+++` or `rename to` refine it.
      const names = line.slice("diff --git ".length);
      const split = names.lastIndexOf(" b/");
      files.set(stripPrefix(split >= 0 ? names.slice(split + 1) : names), current);
      continue;
    }
    if (!current) continue;
    if (hunk === null) {
      const set = (path: string) => {
        for (const [key, value] of files) if (value === current) files.delete(key);
        files.set(path, current as { binary: boolean; hunks: Hunk[] });
      };
      if (line.startsWith("--- ")) {
        oldPath = line.slice(4) === "/dev/null" ? null : stripPrefix(line.slice(4));
      } else if (line.startsWith("+++ ")) {
        const target = line.slice(4);
        if (target !== "/dev/null") set(stripPrefix(target));
        else if (oldPath !== null) set(oldPath);
      } else if (line.startsWith("rename to ")) {
        set(unquote(line.slice("rename to ".length)));
      } else if (line.startsWith("Binary files ")) {
        current.binary = true;
      }
    }
    const header = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/.exec(line);
    if (header) {
      oldLine = Number(header[1]);
      newLine = Number(header[3]);
      hunk = {
        oldStart: oldLine,
        newStart: newLine,
        newCount: header[4] === undefined ? 1 : Number(header[4]),
        lines: [],
      };
      // An empty side starts at 0: its first line is the next one.
      if (header[2] === "0") oldLine += 1;
      if (header[4] === "0") newLine += 1;
      current.hunks.push(hunk);
      continue;
    }
    if (!hunk) continue;
    const sign = line[0];
    const text = line.slice(1);
    if (sign === " ") hunk.lines.push({ kind: "context", old: oldLine++, new: newLine++, text });
    else if (sign === "-") hunk.lines.push({ kind: "del", old: oldLine++, new: null, text });
    else if (sign === "+") hunk.lines.push({ kind: "add", old: null, new: newLine++, text });
  }
  return files;
}

type Hunk = { oldStart: number; newStart: number; newCount: number; lines: DiffLine[] };

let nextGap = 0;

/** A hunk's lines with long unchanged stretches folded, keeping `KEEP` lines by each change. */
function fold(lines: DiffLine[], leading: boolean, trailing: boolean): DiffRow[] {
  const rows: DiffRow[] = [];
  let index = 0;
  while (index < lines.length) {
    const line = lines[index] as DiffLine;
    if (line.kind !== "context") {
      rows.push(line);
      index += 1;
      continue;
    }
    let end = index;
    while (end < lines.length && (lines[end] as DiffLine).kind === "context") end += 1;
    const run = lines.slice(index, end);
    const keepBefore = index === 0 && leading ? 0 : KEEP;
    const keepAfter = end === lines.length && trailing ? 0 : KEEP;
    if (run.length > keepBefore + keepAfter + 1) {
      rows.push(...run.slice(0, keepBefore));
      const hidden = run.slice(keepBefore, run.length - keepAfter);
      rows.push({ kind: "gap", id: nextGap++, count: hidden.length, lines: hidden });
      rows.push(...run.slice(run.length - keepAfter));
    } else rows.push(...run);
    index = end;
  }
  return rows;
}

/** The part of `patch` that changes `path`: its own `diff --git` section. */
export function filePatch(patch: string, path: string): string {
  const sections = patch.split(/^(?=diff --git )/m);
  return sections.filter((section) => readFiles(section).has(path)).join("");
}

/**
 * The files of `patch`, in the order of `paths`. With `wholeFiles` each hunk is its whole
 * file, so every folded stretch can be opened; otherwise the stretches between hunks are only
 * counted.
 */
export function parsePatch(patch: string, paths: string[], wholeFiles: boolean): PatchFile[] {
  const files = readFiles(patch);
  return paths.map((path) => {
    const file = files.get(path);
    if (!file) return { path, binary: false, rows: [] };
    const rows: DiffRow[] = [];
    let nextNew = 1;
    file.hunks.forEach((hunk, index) => {
      const skipped = hunk.newStart - nextNew;
      if (!wholeFiles && skipped > 0 && hunk.newCount > 0) {
        rows.push({ kind: "gap", id: nextGap++, count: skipped, lines: null });
      }
      rows.push(
        ...(wholeFiles ? fold(hunk.lines, index === 0, index === file.hunks.length - 1) : hunk.lines),
      );
      nextNew = hunk.newStart + hunk.newCount;
    });
    return { path, binary: file.binary, rows };
  });
}

/** A deleted line and the added line that replaced it: the part that differs, by offsets. */
export function changedSpan(before: string, after: string): { before: [number, number]; after: [number, number] } {
  let start = 0;
  const limit = Math.min(before.length, after.length);
  while (start < limit && before[start] === after[start]) start += 1;
  let end = 0;
  while (
    end < limit - start &&
    before[before.length - 1 - end] === after[after.length - 1 - end]
  ) {
    end += 1;
  }
  return {
    before: [start, before.length - end],
    after: [start, after.length - end],
  };
}

/** The paths `patch` changes, in its order. */
export function patchPaths(patch: string): string[] {
  return [...readFiles(patch).keys()];
}

/**
 * One file's diff text as rows: a git hunk (`@@ -3,7 +3,9 @@`) gives each line its numbers, and
 * the unchanged stretch between two hunks is a gap; bare `-`/`+` lines (an edit rebuilt from its
 * call's old and new text) have none. `numbered` says which it was.
 */
export function parseDiff(diff: string): { numbered: boolean; rows: DiffRow[] } {
  const lines = diff.replace(/\n$/, "").split("\n").filter((line) => !/^(---|\+\+\+) /.test(line) && !line.startsWith("diff --git "));
  if (lines.some((line) => /^@@ -\d+(?:,\d+)? \+\d+(?:,\d+)? @@/.test(line))) {
    const [file] = parsePatch(`diff --git a/f b/f\n${lines.join("\n")}\n`, ["f"], false);
    // The lines before the first change are not shown apart: only those between changes are.
    const rows = file?.rows ?? [];
    return { numbered: true, rows: rows[0]?.kind === "gap" ? rows.slice(1) : rows };
  }
  const rows: DiffRow[] = lines.flatMap((line): DiffRow[] => {
    const sign = line[0];
    const text = line.slice(1);
    if (sign === "+") return [{ kind: "add", old: null, new: null, text }];
    if (sign === "-") return [{ kind: "del", old: null, new: null, text }];
    if (sign === " ") return [{ kind: "context", old: null, new: null, text }];
    // A hunk header without numbers (`@@`): the edit's places apart.
    if (line.startsWith("@@")) return [{ kind: "gap", id: nextGap++, count: 0, lines: null }];
    return [];
  });
  return { numbered: false, rows };
}

/** How many lines rows add and remove. */
export function diffCounts(rows: readonly DiffRow[]): { added: number; removed: number } {
  let added = 0;
  let removed = 0;
  for (const row of rows) {
    if (row.kind === "add") added += 1;
    else if (row.kind === "del") removed += 1;
  }
  return { added, removed };
}

/** Words, runs of spaces and single marks: what a word diff compares. */
function tokens(text: string): { text: string; at: number }[] {
  return [...text.matchAll(/\w+|\s+|[^\w\s]/g)].map((match) => ({ text: match[0], at: match.index }));
}

/** Lines longer than this in tokens are compared by `changedSpan` instead: a word diff costs their product. */
const WORD_DIFF_TOKENS = 300;

/**
 * A deleted line and the added line that replaced it: the words that differ on each side, as
 * offset ranges, from the longest run of words they share. Spaces between two changed words join
 * them into one range.
 */
export function wordSpans(before: string, after: string): { before: [number, number][]; after: [number, number][] } {
  const a = tokens(before);
  const b = tokens(after);
  if (a.length > WORD_DIFF_TOKENS || b.length > WORD_DIFF_TOKENS) {
    const span = changedSpan(before, after);
    return {
      before: span.before[0] < span.before[1] ? [span.before] : [],
      after: span.after[0] < span.after[1] ? [span.after] : [],
    };
  }
  // The longest common sequence of tokens, by its lengths from each end.
  const lengths = Array.from({ length: a.length + 1 }, () => Array.from({ length: b.length + 1 }, () => 0));
  for (let i = a.length - 1; i >= 0; i--) {
    for (let j = b.length - 1; j >= 0; j--) {
      lengths[i]![j] = a[i]!.text === b[j]!.text ? lengths[i + 1]![j + 1]! + 1 : Math.max(lengths[i + 1]![j]!, lengths[i]![j + 1]!);
    }
  }
  const keptA = new Set<number>();
  const keptB = new Set<number>();
  for (let i = 0, j = 0; i < a.length && j < b.length; ) {
    if (a[i]!.text === b[j]!.text) {
      keptA.add(i++);
      keptB.add(j++);
    } else if (lengths[i + 1]![j]! >= lengths[i]![j + 1]!) i++;
    else j++;
  }
  return { before: ranges(a, keptA), after: ranges(b, keptB) };
}

const space = (token: { text: string }) => /^\s+$/.test(token.text);

/** The changed tokens as ranges, joined across the spaces between them. */
function ranges(side: { text: string; at: number }[], kept: ReadonlySet<number>): [number, number][] {
  const out: [number, number][] = [];
  let previous = -1;
  side.forEach((token, index) => {
    if (kept.has(index) || space(token)) return;
    const end = token.at + token.text.length;
    const last = out.at(-1);
    if (last && side.slice(previous + 1, index).every(space)) last[1] = end;
    else out.push([token.at, end]);
    previous = index;
  });
  return out;
}
