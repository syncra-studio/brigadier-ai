import type { LimitHit, QuotaWait } from "@/ipc/generated";
import { formatDateTime, formatTime } from "@/lib/format";

/** Only the scheduler's recovery mode can promise an automatic attempt; a reset alone cannot. */
export function quotaRecoveryWords(wait: QuotaWait): { title: string; detail: string | null; cancelLabel: string | null } {
  const credits = wait.limitKind === "credits";
  const spend = wait.limitKind === "spendControl";
  const recovery = wait.recovery;
  const title = credits ? "Out of credits" : spend ? "Spend limit reached" : "Usage limit reached";
  if (credits || spend || recovery?.mode === "manualOnly") {
    return { title, detail: credits ? "Continue after credits are available." : spend ? "Continue after the spend limit is updated." : "Continue when usage is available.", cancelLabel: null };
  }
  if (recovery?.mode === "automaticAt" || recovery?.mode === "automaticProbe") {
    const at = recovery.nextCheckAtMs;
    const checking = at === null ? "Checking usage automatically."
      : recovery.mode === "automaticAt" ? `Will try again automatically at ${formatDateTime(at)}.`
      : `Checking again at ${formatDateTime(at)}.`;
    return { title, detail: `${checking} Work resumes automatically when usage is available.`, cancelLabel: "Don't resume" };
  }
  // Old records contain no durable scheduling promise. Keep their reset information truthful.
  return { title: wait.resetsAtMs === null ? `${title} · Reset time unavailable` : `${title} · Resets ${formatDateTime(wait.resetsAtMs)}`,
    detail: null, cancelLabel: "Stop waiting" };
}

/** What the "Resumes at" card says, or `null` when there is none. */
export type ResumesAt = {
  title: string;
  reason: string | null;
  atMs: number | null;
  /** Whether it resumes on its own, so Don't resume can stop that. */
  resumes: boolean;
};

/**
 * The "Resumes at" card (MA5), from the session's last turn's limit as its CLI said it and the
 * session's wait: a usage window with a resume scheduled reads "Resumes at HH:MM"; a spend limit
 * or credits that ran out say so, with no time: they never resume on their own. With no wait (Don't
 * resume, or none scheduled) and no such limit, nothing.
 */
export function resumesAt(limit: LimitHit | null, wait: QuotaWait | null): ResumesAt | null {
  const kind = wait ? wait.limitKind ?? null : limit?.kind ?? null;
  if (kind === "credits" || kind === "spendControl") {
    return { title: kind === "credits" ? "Out of credits" : "Spend limit reached", reason: wait?.reason ?? null, atMs: null, resumes: false };
  }
  if (!wait) return null;
  // A current wait with no known reset must never inherit an older turn's reset.
  const atMs = wait.resetsAtMs;
  return { title: atMs !== null ? `Resumes at ${formatTime(atMs)}` : "Resumes when the limit resets", reason: wait.reason, atMs, resumes: true };
}
