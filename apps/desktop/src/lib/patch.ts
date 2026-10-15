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
  /** Git's "\\ No newline at end of file" follows it: its side's file ends without one. */
  noEol?: true;
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
    else if (sign === "\\") {
      const last = hunk.lines.at(-1);
      if (last) last.noEol = true;
    }
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

/**
 * One file's diff text as a whole single-file patch the diff view reads. Git's hunks keep their
 * numbers. Bare `-`/`+` lines (an edit rebuilt from its call's old and new text) get made-up
 * hunks one line apart, so each edit stands apart and is drawn without numbers.
 */
export function singleFilePatch(path: string, diff: string): string {
  const lines = diff.replace(/\n$/, "").split("\n");
  const deleted = lines.some((line) => line === "+++ /dev/null" || line.startsWith("deleted file mode"));
  const created = lines.some((line) => line === "--- /dev/null" || line.startsWith("new file mode"));
  const body = lines.filter((line) => !/^(---|\+\+\+) /.test(line) && !line.startsWith("diff --git ") && !/^(index|new file mode|deleted file mode|similarity|rename) /.test(line));
  const head = `diff --git a/${path} b/${path}\n--- ${created ? "/dev/null" : `a/${path}`}\n+++ ${deleted ? "/dev/null" : `b/${path}`}\n`;
  if (body.some((line) => /^@@ -\d+(?:,\d+)? \+\d+(?:,\d+)? @@/.test(line))) return `${head}${body.join("\n")}\n`;
  const hunks: string[][] = [[]];
  for (const line of body) {
    if (line.startsWith("@@")) hunks.push([]);
    else if (/^[ +-]/.test(line)) hunks.at(-1)?.push(line);
  }
  let oldAt = 1;
  let newAt = 1;
  const out: string[] = [];
  for (const hunk of hunks.filter((kept) => kept.length > 0)) {
    const olds = hunk.filter((line) => line[0] !== "+").length;
    const news = hunk.filter((line) => line[0] !== "-").length;
    // An empty side starts at the line before it, as git writes it.
    out.push(`@@ -${olds === 0 ? oldAt - 1 : oldAt},${olds} +${news === 0 ? newAt - 1 : newAt},${news} @@`, ...hunk);
    oldAt += olds + 1;
    newAt += news + 1;
  }
  return `${head}${out.join("\n")}\n`;
}

/** Whether a file's diff text deletes the file. */
export function deletesFile(diff: string): boolean {
  return /^(\+\+\+ \/dev\/null|deleted file mode)/m.test(diff);
}

/**
 * Both sides of a file whose patch holds all of it (whole files loaded): every line of the old
 * file and of the new one, or `null` when a stretch was only counted.
 */
export function fileSides(rows: readonly DiffRow[]): { before: string; after: string } | null {
  const before: DiffLine[] = [];
  const after: DiffLine[] = [];
  for (const row of rows) {
    const shown = row.kind === "gap" ? row.lines : [row];
    if (!shown) return null;
    for (const line of shown) {
      if (line.kind !== "add") before.push(line);
      if (line.kind !== "del") after.push(line);
    }
  }
  // A side whose last line git marked keeps its missing final newline, so a change of only that
  // newline still differs.
  const side = (lines: DiffLine[]) =>
    lines.length ? `${lines.map((line) => line.text).join("\n")}${lines.at(-1)?.noEol ? "" : "\n"}` : "";
  return { before: side(before), after: side(after) };
}
