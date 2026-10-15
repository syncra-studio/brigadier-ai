import type { Notice, UserRequest } from "@/ipc/generated";

/**
 * A failed turn in plain words. The thread says what happened in one calm line; the error's own
 * words (an API's JSON, a CLI's exit) stay under Details. The line comes from the raw text, so
 * old turns read the same as new ones.
 */

/** The daemon's words when the CLI's process went away mid-turn (`ENDED_UNEXPECTEDLY`). */
const ENDED_UNEXPECTEDLY = /cli session ended unexpectedly|ended unexpectedly|\bkilled\b|crash|terminated by signal|\bsig(kill|term|segv|abrt)\b/i;
const LIMIT = /usage limit|hit your limit|limit (was )?reached|limit exceeded|quota|out of (usage|credits)/i;
const NETWORK = /network|connection|disconnected|\beconn\w*|enetunreach|enotfound|etimedout|eai_again|timed? ?out|timeout|fetch failed|socket hang up|getaddrinfo|offline/i;
const API = /api error|\b[45]\d\d\b|overloaded|server error|bad gateway|service unavailable|invalid_request|authentication_error/i;

/** The daemon's words for a turn that was running when Brigadier quit (`INTERRUPTED_BY_QUIT`). */
const INTERRUPTED = "Brigadier quit while this was running.";

/** Whether a turn ended because Brigadier quit while it ran: it reads interrupted, with Continue. */
export function interruptedByQuit(raw: string | null | undefined): boolean {
  return raw?.trim() === INTERRUPTED;
}

/** `limit`: the provider's usage limit stopped it, which the thread shows as a divider, not an error. */
export function describeTurnError(raw: string, provider = "Claude"): { summary: string; limit?: true } {
  if (interruptedByQuit(raw)) return { summary: "Interrupted: Brigadier quit while this was running. Continue picks it up where it stands." };
  if (LIMIT.test(raw)) return { summary: `${provider}'s usage limit was reached.`, limit: true };
  if (ENDED_UNEXPECTEDLY.test(raw)) return { summary: `${provider} stopped unexpectedly.` };
  if (NETWORK.test(raw)) return { summary: `Lost the connection to ${provider}.` };
  if (API.test(raw)) return { summary: `This turn stopped because of an error on ${provider}'s side.` };
  return { summary: "This turn stopped because of an error." };
}

/** The error of the conversation's newest failed request, which its turn shows in the thread. */
export function newestFailedError(requests: Readonly<Record<string, UserRequest>>): string | null {
  let newest: UserRequest | null = null;
  for (const request of Object.values(requests)) {
    if (request.state.type !== "failed") continue;
    if (!newest || request.startedAtMs > newest.startedAtMs) newest = request;
  }
  return newest?.state.type === "failed" ? newest.state.error : null;
}

/**
 * The notices left to show above the composer: one that repeats the failed turn's error goes, as
 * the turn already says it (conversations stored before the daemon stopped sending it).
 */
export function withoutTurnError<T extends Pick<Notice, "text">>(notices: readonly T[], error: string | null): readonly T[] {
  const said = error?.trim();
  if (!said) return notices;
  return notices.some((notice) => notice.text.trim() === said) ? notices.filter((notice) => notice.text.trim() !== said) : notices;
}
