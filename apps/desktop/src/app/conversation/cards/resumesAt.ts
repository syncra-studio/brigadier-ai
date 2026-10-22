import type { LimitHit, QuotaWait } from "@/ipc/generated";
import { formatTime } from "@/lib/format";

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
  const kind = wait?.limitKind ?? limit?.kind ?? null;
  if (kind === "credits" || kind === "spendControl") {
    return { title: kind === "credits" ? "Out of credits" : "Spend limit reached", reason: wait?.reason ?? null, atMs: null, resumes: false };
  }
  if (!wait) return null;
  const atMs = wait.resetsAtMs ?? limit?.resetsAtMs ?? null;
  return { title: atMs !== null ? `Resumes at ${formatTime(atMs)}` : "Resumes when the limit resets", reason: wait.reason, atMs, resumes: true };
}
