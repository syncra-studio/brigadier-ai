import type { Notice } from "@/ipc/generated";

/** Older releases stored version drift as a notice. It belongs only in diagnostics. */
export function showConversationNotice(notice: Notice): boolean {
  return !/^Codex \S+ is running; Brigadier[’']s bindings were generated from /u.test(
    notice.text,
  );
}
