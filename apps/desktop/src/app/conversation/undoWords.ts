import type { HeldPath } from "@/ipc/generated";

/**
 * What an Undo or Reapply left as it was, after its toast's first words: ". Left notes.md as it
 * is: you changed it during this request too." One file says why; several are named.
 */
export function leftWords(left: readonly HeldPath[], reapply = false): string {
  const [only] = left;
  if (!only) return "";
  const names = left.map((file) => file.path.split("/").pop() || file.path);
  // Reapply never touched what Undo left, so it doesn't give Undo's reason again.
  if (reapply) {
    return left.length === 1
      ? `. ${names[0]} stays as it is: Undo left it`
      : `. ${left.length} files stay as they are (${names.join(", ")}): Undo left them`;
  }
  if (left.length === 1) return `. ${only.reason}`;
  return `. Left ${left.length} files as they are (${names.join(", ")}): changed by you or by a command during the request`;
}

/** The toast after an Undo or Reapply: what it did, then what it left. One that put nothing back says so. */
export function undoToast(left: readonly HeldPath[], restored: number, reapply: boolean): string {
  const done = restored > 0 ? (reapply ? "Changes reapplied" : "Changes reverted") : reapply ? "Nothing was reapplied" : "Nothing was reverted";
  return `${done}${leftWords(left, reapply)}`;
}
