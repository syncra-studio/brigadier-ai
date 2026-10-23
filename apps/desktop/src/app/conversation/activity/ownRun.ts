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
