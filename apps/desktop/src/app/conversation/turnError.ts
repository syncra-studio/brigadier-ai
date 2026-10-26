import type { Notice, ThreadTurn, UserRequest } from "@/ipc/generated";

/**
 * A failed turn in plain words. The thread says what happened in one calm line; the error's own
 * words (an API's JSON, a CLI's exit) stay under Details. The line comes from the raw text, so
 * old turns read the same as new ones.
 */

/** The daemon's words when the CLI's process went away mid-turn (`ENDED_UNEXPECTEDLY`). */
const ENDED_UNEXPECTEDLY = /cli session ended unexpectedly|ended unexpectedly|\bkilled\b|crash|terminated by signal|\bsig(kill|term|segv|abrt)\b/i;
const RATE = /rate.?limit|too many requests|\b429\b/i;
const CONTEXT = /context (window|length)|prompt is too long|too long for the (model|context)|maximum context|context_length_exceeded|out of context/i;
const AUTH = /not (logged|signed) in|log ?in again|sign ?in again|unauthori[sz]ed|\b401\b|invalid (api|x-api) key|authentication_error|oauth token (has )?expired/i;
const OVERLOADED = /overloaded|\b529\b/i;
const LIMIT = /usage limit|hit your (?:(?:session|weekly) )?limit|limit (was )?reached|limit exceeded|quota|out of (usage|credits)/i;
const NETWORK = /network|connection|disconnected|\beconn\w*|enetunreach|enotfound|etimedout|eai_again|timed? ?out|timeout|fetch failed|socket hang up|getaddrinfo|offline/i;
const API = /api error|\b[45]\d\d\b|overloaded|server error|bad gateway|service unavailable|invalid_request|authentication_error/i;

/** The daemon's words for a turn that was running when Brigadier quit (`INTERRUPTED_BY_QUIT`). */
export const INTERRUPTED = "Brigadier quit while this was running.";
/** …and for one still starting, whose message the thread never had (`INTERRUPTED_BEFORE_START`). */
const INTERRUPTED_BEFORE_START = "Brigadier quit before this started.";

/** Whether a turn ended because Brigadier quit while it ran or started: it reads interrupted, with Continue. */
export function interruptedByQuit(raw: string | null | undefined): boolean {
  const text = raw?.trim();
  return text === INTERRUPTED || text === INTERRUPTED_BEFORE_START;
}

/** The daemon's words when a turn's CLI went away and Brigadier didn't quit (`ENDED_UNEXPECTEDLY`). */
const CLI_ENDED = "The CLI session ended unexpectedly.";

type RequestRecord = Pick<UserRequest, "id" | "state">;

/**
 * An interrupted turn read by its requests as the daemon has them: the thread says a turn its
 * CLI never ended was cut by a quit, which only its requests can tell. `error`: the words its
 * block says, a request's own when it failed for another reason (or the CLI's end, when none
 * failed: a turn of Brigadier's own); `resumable`: Continue picks it up, as the daemon's
 * resume does only for the newest request, stopped or cut by a quit.
 */
export function cutOff(error: string, own: readonly RequestRecord[], latest: RequestRecord | null): { error: string; resumable: boolean } {
  if (!interruptedByQuit(error)) return { error, resumable: false };
  const quit = (request: RequestRecord) => request.state.type === "failed" && interruptedByQuit(request.state.error);
  const resumable = !!latest && own.some((request) => request.id === latest.id) && (latest.state.type === "stopped" || quit(latest));
  const cut = own.findLast(quit);
  if (cut?.state.type === "failed") return { error: cut.state.error, resumable };
  if (resumable) return { error, resumable };
  const failed = own.findLast((request) => request.state.type === "failed");
  return { error: failed?.state.type === "failed" ? failed.state.error : CLI_ENDED, resumable: false };
}

/** `limit`: the provider's usage limit stopped it, which the thread shows as a divider, not an error. */
export function describeTurnError(raw: string, provider = "Claude"): { summary: string; limit?: true } {
  if (raw.trim() === INTERRUPTED_BEFORE_START) return { summary: "Interrupted: Brigadier quit before this started. Continue sends it again." };
  if (interruptedByQuit(raw)) return { summary: "Interrupted: Brigadier quit while this was running. Continue picks it up where it stands." };
  // A rate limit reads "limit exceeded" too, and is no usage limit: it passes in a moment.
  if (RATE.test(raw)) return { summary: `${provider} is limiting how fast it takes requests. Try again in a moment.` };
  if (LIMIT.test(raw)) return { summary: `${provider}'s usage limit was reached.`, limit: true };
  if (CONTEXT.test(raw)) return { summary: `The conversation grew too long for ${provider}'s context.` };
  if (AUTH.test(raw)) return { summary: `${provider} needs you to sign in again.` };
  if (ENDED_UNEXPECTEDLY.test(raw)) return { summary: `${provider} stopped unexpectedly.` };
  if (NETWORK.test(raw)) return { summary: `Lost the connection to ${provider}.` };
  if (OVERLOADED.test(raw)) return { summary: `${provider} is overloaded right now.` };
  if (API.test(raw)) return { summary: `This turn stopped because of an error on ${provider}'s side.` };
  return { summary: "This turn stopped because of an error." };
}

/** The error of the conversation's newest failed turn, as its CLI's turns say it: its turn shows it in the thread. */
export function newestFailedError(turns: readonly Pick<ThreadTurn, "end" | "error">[]): string | null {
  return turns.findLast((turn) => turn.end === "failed" || turn.end === "interrupted")?.error ?? null;
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
