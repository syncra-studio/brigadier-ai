/*
 * Account emails stay out of sight: the first three letters of who, the rest dots, the
 * domain's ending kept (`ste•••••@•••••.com`). Settings › Providers reveals one row's on
 * request; everywhere else they stay masked.
 */

const DOTS = "•••••";

/** Words shaped like an email, wherever they sit in a line of text. */
const EMAIL = /[^\s@;,()<>"']+@[^\s@;,()<>"']+\.[A-Za-z]{2,}/g;

/** Whether `text` holds an email (an account named after one is masked like one). */
export function hasEmail(text: string): boolean {
  return text.match(EMAIL) !== null;
}

/** `stephen@example.com` → `ste•••••@•••••.com`; text without an `@` comes back as it is. */
export function maskEmail(email: string): string {
  const at = email.lastIndexOf("@");
  if (at < 0) return email;
  const who = email.slice(0, at);
  const domain = email.slice(at + 1);
  const dot = domain.lastIndexOf(".");
  const ending = dot >= 0 ? domain.slice(dot) : "";
  return `${who.slice(0, 3)}${DOTS}@${DOTS}${ending}`;
}

/** `text` with every email in it masked. */
export function maskEmailsIn(text: string): string {
  return text.replace(EMAIL, (email) => maskEmail(email));
}
