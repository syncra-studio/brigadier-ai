/** Brigadier's own command tools, whose reply starts with how the command ended: `[exit 0]`. */
export const OWN_RUNS = new Set(["run", "run_unsandboxed", "run_check"]);

const EXIT_LINE = /^\[exit (-?\d+)\]\r?\n/;

/**
 * An own command's reply as its exit code and what it printed: the `[exit N]` line is read, not
 * shown. Any other first line (`[killed by signal 9]`) stays, as what there is to say.
 */
export function ownRunOutput(output: string | null): { exit: number | null; output: string | null } {
  const found = output ? EXIT_LINE.exec(output) : null;
  return found && output ? { exit: Number(found[1]), output: output.slice(found[0].length) } : { exit: null, output };
}

const CLI_EXIT_LINE = /^Exit code (-?\d+)(?:\r?\n|$)/;

/**
 * A Claude CLI `Bash` call's reply as its exit code and what it printed: a failed command's reply
 * starts with `Exit code N`, read, not shown, as the box ends with it already.
 */
export function cliRunOutput(output: string | null): { exit: number | null; output: string | null } {
  const found = output ? CLI_EXIT_LINE.exec(output) : null;
  return found && output ? { exit: Number(found[1]), output: output.slice(found[0].length) } : { exit: null, output };
}
