import type { Notice } from "@/ipc/generated";
import { maskEmailsIn } from "@/lib/email";

/** Older releases stored version drift as a notice. It belongs only in diagnostics. */
export function showConversationNotice(notice: Notice): boolean {
  return !/^Codex \S+ is running; Brigadier[’']s bindings were generated from /u.test(
    notice.text,
  );
}

/**
 * A notice as the thread shows it: an account switch names the accounts, and one saved before
 * emails were masked may carry a whole email, so every email in it is masked (what is stored
 * stays as it was).
 */
export function shownNotice(notice: Notice): Notice {
  const text = maskEmailsIn(notice.text);
  return text === notice.text ? notice : { ...notice, text };
}
