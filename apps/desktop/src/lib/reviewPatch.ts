import type { PendingHunk } from "@/ipc/generated";

/** A file's lines, each with its line ending, as git counts them. */
function lines(text: string): string[] {
  return text.match(/[^\n]*\n|[^\n]+$/g) ?? [];
}

/** A hunk header's side over a whole file of `count` lines. */
function range(count: number): string {
  return `${count === 0 ? 0 : 1},${count}`;
}

/** Where a change sits in its file: the first old line it replaces, or the new line it adds first. */
export function changeLine(hunk: PendingHunk): { side: "additions" | "deletions"; lineNumber: number } {
  return hunk.oldLines > 0 ? { side: "deletions", lineNumber: hunk.oldStart } : { side: "additions", lineNumber: hunk.newStart };
}

/**
 * A file under review as one patch over all of it: every unchanged line as context and each
 * pending change's old lines, then its new ones. The diff then draws exactly the changes Keep and
 * Undo take, numbered as the engine numbers them.
 */
export function reviewPatch(path: string, base: string | null, current: string | null, hunks: readonly PendingHunk[]): string {
  const old = lines(base ?? "");
  const now = lines(current ?? "");
  const out: string[] = [];
  const put = (mark: string, line: string) => {
    out.push(mark + line.replace(/\n$/, ""));
    if (!line.endsWith("\n")) out.push("\\ No newline at end of file");
  };
  let at = 0;
  for (const hunk of hunks) {
    const from = hunk.oldLines === 0 ? hunk.oldStart : hunk.oldStart - 1;
    while (at < from) put(" ", old[at++]!);
    for (let count = 0; count < hunk.oldLines; count++) put("-", old[at++]!);
    const added = hunk.newLines === 0 ? hunk.newStart : hunk.newStart - 1;
    for (let count = 0; count < hunk.newLines; count++) put("+", now[added + count]!);
  }
  while (at < old.length) put(" ", old[at++]!);
  return [`--- a/${path}`, `+++ b/${path}`, `@@ -${range(old.length)} +${range(now.length)} @@`, ...out, ""].join("\n");
}
