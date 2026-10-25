/**
 * What of a reply still streaming is drawn (D9 b): only its finished lines, so its Markdown is
 * never drawn half-built and then redrawn. A table shows once its header and the divider under it
 * are both there, then a row at a time; a code block grows a line at a time. The reply's own end
 * brings its whole text.
 */
export function releasedText(text: string): string {
  const end = text.lastIndexOf("\n");
  if (end < 0) return "";
  const finished = text.slice(0, end + 1);
  return opensTable(finished) ? finished.slice(0, finished.lastIndexOf("\n", end - 1) + 1) : finished;
}

/** A streaming text as drawn: its finished lines while it streams, whole once it ended. */
export function shownText(text: string, streaming: boolean): string {
  return streaming ? releasedText(text) : text;
}

/** A fence's opening line, after any quote or list indent: its run of backticks or tildes. */
const FENCE = /^[\s>]*(`{3,}|~{3,})/;
/** A line that may be a table's row (the header and divider are rows too). */
const ROW = /\|/;

/**
 * Whether the last of `finished`'s lines may be a table's header still waiting for its divider:
 * a row outside code, with no row above it.
 */
function opensTable(finished: string): boolean {
  const lines = finished.split("\n");
  lines.pop();
  let fence: string | null = null;
  let above = "";
  let last = "";
  let inCode = false;
  for (const line of lines) {
    above = last;
    last = line;
    const run = FENCE.exec(line)?.[1];
    inCode = fence !== null || run !== undefined;
    if (fence === null) {
      if (run !== undefined) fence = run;
    } else if (run !== undefined && run[0] === fence[0] && run.length >= fence.length && line.replace(/^[\s>]*/, "").trimEnd() === run) {
      fence = null;
    }
  }
  return !inCode && ROW.test(last) && !ROW.test(above);
}
