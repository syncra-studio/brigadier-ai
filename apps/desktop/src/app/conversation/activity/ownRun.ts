/** Brigadier's own command tools, whose reply starts with how the command ended: `[exit 0]`. */
export const OWN_RUNS = new Set(["run", "run_unsandboxed", "run_check"]);

/** A short reply's first line: `[exit 0]`. */
const EXIT_LINE = /^\[exit (-?\d+)\]\r?\n/;
/** A long reply's first line, its digest's header: `exit 0 [full output: read_artifact out-…, 120 lines, 6000 bytes]`. */
const DIGEST_LINE = /^exit (-?\d+) \[full output: [^\]\n]*\](?:\r?\n|$)/;
/** What `run_check`'s `commit` adds on a line of its own after the output. */
const COMMIT_LINE = /\n(Committed [0-9a-f]+ ".*" with your trailer: .*|Not committed: .*|Nothing to commit: .*)$/;
/** What a check that starts the review of the thread's change adds after a blank line. */
const REVIEW_NOTE = /\n\n\[review running\] (\S+) reviews your change[^\n]*$/;

/**
 * An own command's reply as its exit code, what it printed, and what Brigadier said about it
 * after the output (the commit `run_check` made, a review it started), in plain words: the
 * `[exit N]` line or a digest's `exit N [full output: …]` header is read, not shown. Any other
 * first line (`[killed by signal 9]`) stays, as what there is to say.
 */
export function ownRunOutput(output: string | null): { exit: number | null; output: string | null; notes: string[] } {
  if (!output) return { exit: null, output, notes: [] };
  let rest = output.replace(/\r?\n$/, "");
  const notes: string[] = [];
  const review = REVIEW_NOTE.exec(rest);
  if (review) {
    notes.unshift(`${review[1] ?? "A reviewer"} is reviewing this change.`);
    rest = rest.slice(0, review.index);
  }
  const commit = COMMIT_LINE.exec(rest);
  if (commit) {
    notes.unshift((commit[1] ?? "").replace(/ with your trailer:/, ":"));
    rest = rest.slice(0, commit.index + 1);
  }
  if (notes.length === 0) rest = output;
  const found = EXIT_LINE.exec(rest) ?? DIGEST_LINE.exec(rest);
  return found ? { exit: Number(found[1]), output: rest.slice(found[0].length), notes } : { exit: null, output: rest, notes };
}

const CLI_EXIT_LINE = /^Exit code (-?\d+)(?:\r?\n|$)/;

/**
 * A CLI command's reply as its exit code and what it printed: the Claude CLI's failed command
 * (its `Bash`, a `shell` step here) replies `Exit code N` first, read, not shown, as the box ends
 * with it already.
 */
export function cliRunOutput(output: string | null): { exit: number | null; output: string | null } {
  const found = output ? CLI_EXIT_LINE.exec(output) : null;
  return found && output ? { exit: Number(found[1]), output: output.slice(found[0].length) } : { exit: null, output };
}
