import type { Notice } from "@/ipc/generated";
import { maskEmailsIn } from "@/lib/email";

/** Older releases stored version drift as a notice. It belongs only in diagnostics. */
const VERSION_DRIFT = /^Codex \S+ is running; Brigadier[’']s bindings were generated from /u;

/**
 * Usage limits and moves to another model, provider or account that older releases stored as
 * notices. The thread says them as dividers now (#506), so a saved one is never said again.
 */
const LIMIT_OR_SWITCH = [
  /hit your .*limit/iu,
  /; continuing (?:on|with) /u,
  /; task \d+ continues on /u,
  /Your message waits and goes on its own once a model you allow can take it\.$/u,
];

/** Whether a notice shows above the composer; stored ones that belong elsewhere don't. */
export function showConversationNotice(notice: Notice): boolean {
  return !VERSION_DRIFT.test(notice.text) && !LIMIT_OR_SWITCH.some((pattern) => pattern.test(notice.text));
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
