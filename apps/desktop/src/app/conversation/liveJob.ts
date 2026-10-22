import { useMemo } from "react";

import { ciPill, ciWatches, shortSha } from "@/app/conversation/activity/slotView";
import { isRunRequest, isWorking } from "@/app/conversation/blocks";
import { questionOpen } from "@/app/conversation/cards/questionRound";
import type { OrchestratorStep } from "@/ipc/generated";
import { type Board, liveRequest, useBoard } from "@/state/board";

/**
 * The Job card as a live summary of the session right now (J1): one headline and one detail
 * line; the buttons are the Background rows' (Open CI, Cancel). The thread's own
 * headline (`set_headline`) gives the words (J2), and says whether they tell of a failure;
 * before it sets one, or when something that needs the user or failed came after it, the board's
 * events give them, in this order (J5): waiting on you, something failed, something running, done. Running and waiting things show whenever they started; a finished outcome only
 * until the user's next message (J4). It names no worker or Background row: those are the
 * context card's, and the Workers strip's.
 */

/** Which icon the card shows: a spinner, a raised hand, a red cross, a tick, a stop, a merge. */
export type LiveTone = "running" | "waiting" | "failed" | "done" | "stopped" | "merged";

export type LiveJob = {
  tone: LiveTone;
  headline: string;
  detail?: string | undefined;
  /** When what it tells began: a running state's timer counts from it. */
  sinceMs?: number | undefined;
  /** When it ended: an outcome shows how long it took. */
  endedAtMs?: number | undefined;
  /** Only the bare "Working": nothing more specific to say (a run's or plan's card says its own). */
  plain?: true | undefined;
};

/** The board's parts it reads; `headline` is the thread's (`set_headline`), cleared by the user's next message. */
export type LiveInput = Pick<
  Board,
  "tasks" | "approvals" | "questions" | "previews" | "orchestratorSteps" | "requests"
> &
  Partial<Pick<Board, "headline">>;

type Kind = "waiting" | "failed" | "running" | "done";

/** One thing the board says, with when it happened and, for running ones, how much it says. */
type Fact = LiveJob & { kind: Kind; atMs: number; rank: number };

const short = (sha: string) => sha.slice(0, 7);
const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;
const firstLine = (text: string | null | undefined) => text?.split("\n").find((line) => line.trim())?.trim() || undefined;

function ciFacts(steps: readonly OrchestratorStep[]): Fact[] {
  const all = ciWatches(steps.filter((step) => !isRunRequest(step.requestId)));
  const live = all.filter((watch) => watch.result === null);
  // While a push is checked, its watch is the card's; once none is, the newest one's outcome.
  const watches = live.length > 0 ? live : all.slice(-1);
  const newest = watches.at(-1);
  if (!newest) return [];
  const pill = ciPill(watches);
  const detail = `${newest.branch} · ${shortSha(newest.sha)}`;
  const sinceMs = watches[0]!.sinceMs;
  const endedAtMs = live.length > 0 ? undefined : (newest.endedAtMs ?? undefined);
  const atMs = endedAtMs ?? sinceMs;
  if (pill.state === "failed") return [{ kind: "failed", rank: 0, tone: "failed", headline: pill.words, detail, sinceMs, endedAtMs, atMs }];
  // Cancelled, none failed: the cancel's own mark, under words that say it.
  if (pill.state === "cancelled") return [{ kind: "failed", rank: 0, tone: "stopped", headline: pill.words, detail, sinceMs, endedAtMs, atMs }];
  if (pill.state === "checking" || live.length > 0) return [{ kind: "running", rank: 5, tone: "running", headline: pill.words, detail, sinceMs, atMs }];
  if (pill.state === "passed") return [{ kind: "done", rank: 0, tone: "done", headline: pill.words, detail, sinceMs, endedAtMs, atMs }];
  return [];
}

function mergeFacts(input: LiveInput, since: number): Fact[] {
  const facts: Fact[] = [];
  const steps = input.orchestratorSteps.filter((step) => !isRunRequest(step.requestId));
  const last = steps.findLast((step) => ["mergeRequested", "merged", "mergeCancelled", "mergeStopped"].includes(step.kind.type));
  const click = liveMergeClick(input, steps, since);
  if (click?.kind.type === "mergeRequested") {
    facts.push({
      kind: "running",
      rank: 7,
      tone: "running",
      headline: `Merging into ${click.kind.base}`,
      detail: `${click.kind.branch} into ${click.kind.base}`,
      sinceMs: click.atMs,
      atMs: click.atMs,
    });
  }
  if (last?.kind.type === "merged") {
    const kind = last.kind;
    const sha = kind.mergeCommit ?? kind.head;
    const detail = [sha && short(sha), kind.pr !== undefined && `#${kind.pr}`, plural(kind.commits, "commit")].filter(Boolean).join(" · ");
    facts.push({
      kind: "done",
      rank: 0,
      tone: "merged",
      headline: `Merged into ${kind.base}`,
      detail,
      atMs: last.atMs,
    });
  }
  return facts;
}

/**
 * The user's merge click still at work: the latest merge step of `steps` is the click, with no
 * outcome yet. A click merges only while the work it set going runs: one the thread turned
 * down, or a stop cut short, is over.
 */
export function liveMergeClick(input: Pick<LiveInput, "requests">, steps: readonly OrchestratorStep[], since: number): OrchestratorStep | null {
  const last = steps.findLast((step) => ["mergeRequested", "merged", "mergeCancelled", "mergeStopped"].includes(step.kind.type));
  if (last?.kind.type !== "mergeRequested") return null;
  const live = liveRequest(input);
  return live !== null && (last.atMs >= since || last.requestId === live.live.id || last.requestId === live.newest) ? last : null;
}

/** When the newest of `items` came. */
const newest = (items: readonly { createdAtMs: number }[]) => Math.max(...items.map((item) => item.createdAtMs));

function waitingFacts(input: LiveInput): Fact[] {
  const facts: Fact[] = [];
  const open = Object.values(input.questions).filter(questionOpen);
  const merges = open.filter((question) => question.kind.type === "merge");
  const others = open.filter((question) => question.kind.type !== "merge");
  if (merges.length > 0) {
    facts.push({ kind: "waiting", rank: 0, tone: "waiting", headline: "Waiting for your yes below", detail: "To merge the work", atMs: newest(merges) });
  }
  if (others.length > 0) {
    facts.push({
      kind: "waiting",
      rank: 0,
      tone: "waiting",
      headline: others.length === 1 ? "Waiting for your answer below" : `Waiting for ${others.length} answers below`,
      atMs: newest(others),
    });
  }
  const approvals = Object.values(input.approvals).filter((approval) => approval.state.type === "pending");
  if (approvals.length > 0) {
    facts.push({
      kind: "waiting",
      rank: 0,
      tone: "waiting",
      headline: approvals.length === 1 ? "Waiting for your approval below" : `Waiting for ${approvals.length} approvals below`,
      atMs: newest(approvals),
    });
  }
  return facts;
}

/** Failures in the work itself (R4): a worker that failed, a dev server or app that died. */
function workFailures(input: LiveInput): Fact[] {
  const facts: Fact[] = [];
  const failed = Object.values(input.tasks)
    .filter((task) => task.state === "failed" && !isRunRequest(task.requestId))
    .toSorted((a, b) => a.updatedAtMs - b.updatedAtMs);
  const last = failed.at(-1);
  if (last) {
    facts.push({
      kind: "failed",
      rank: 0,
      tone: "failed",
      headline: failed.length === 1 ? "A worker failed" : `${failed.length} workers failed`,
      detail: firstLine(last.error),
      atMs: last.updatedAtMs,
    });
  }
  // Each preview's latest run: one restarted since is up again.
  const latest = new Map<string, Board["previews"][string]>();
  for (const preview of Object.values(input.previews).toSorted((a, b) => a.startedAtMs - b.startedAtMs)) latest.set(preview.name, preview);
  for (const preview of latest.values()) {
    if (preview.state.type !== "exited" || preview.state.code === 0) continue;
    facts.push({
      kind: "failed",
      rank: 0,
      tone: "failed",
      headline: `${preview.name} stopped`,
      detail: preview.state.status,
      atMs: preview.endedAtMs ?? preview.startedAtMs,
    });
  }
  return facts;
}

/** The user stopped the work their last message set going. */
function stopFacts(input: LiveInput): Fact[] {
  const last = Object.values(input.requests).reduce<LiveInput["requests"][string] | null>(
    (found, request) => (!found || request.startedAtMs > found.startedAtMs ? request : found),
    null,
  );
  if (last?.state.type !== "stopped") return [];
  const endedAtMs = last.endedAtMs ?? last.startedAtMs;
  return [{ kind: "done", rank: 0, tone: "stopped", headline: "Stopped", detail: "You stopped it", sinceMs: last.startedAtMs, endedAtMs, atMs: endedAtMs }];
}

/** When the user last wrote: finished outcomes from before it are gone (J4). */
export function lastMessageAtMs(input: Pick<LiveInput, "requests">): number {
  return Math.max(Number.NEGATIVE_INFINITY, ...Object.values(input.requests).map((request) => request.startedAtMs));
}

/** The newest of `facts`; among running ones, the one that says the most first. */
function pick(facts: readonly Fact[]): Fact | undefined {
  return facts.toSorted((a, b) => b.rank - a.rank || b.atMs - a.atMs)[0];
}

/** The newest of `facts`. */
function latestOf(facts: readonly Fact[]): Fact | undefined {
  return facts.toSorted((a, b) => b.atMs - a.atMs)[0];
}

export function liveJob(input: LiveInput): LiveJob | null {
  const since = lastMessageAtMs(input);
  const facts = [...waitingFacts(input), ...workFailures(input), ...ciFacts(input.orchestratorSteps), ...mergeFacts(input, since), ...stopFacts(input)];
  // The thread or a worker at work, with nothing more to say: Working, naming no worker (J3).
  const live = liveRequest(input);
  const workers = Object.values(input.tasks).filter((task) => isWorking(task) && !isRunRequest(task.requestId));
  const startedAtMs = live ? (live.live.worked?.at(-1)?.fromMs ?? live.live.startedAtMs) : Math.min(...workers.map((task) => task.createdAtMs));
  if (live || workers.length > 0) {
    facts.push({ kind: "running", rank: 1, tone: "running", headline: "Working", sinceMs: startedAtMs, atMs: startedAtMs, plain: true });
  }
  const waiting = facts.filter((fact) => fact.kind === "waiting");
  // A failure or an outcome from before the user's last message is over with.
  const failed = facts.filter((fact) => fact.kind === "failed" && fact.atMs >= since);
  const running = facts.filter((fact) => fact.kind === "running");
  const done = facts.filter((fact) => fact.kind === "done" && fact.atMs >= since);
  // The bare Working says least: an outcome since the user's last message says more.
  const specific = running.filter((fact) => !fact.plain);
  const shown = pick(waiting) ?? latestOf(failed) ?? pick(specific) ?? latestOf(done) ?? pick(running);
  const headline = input.headline && input.headline.atMs >= since ? input.headline : null;
  // The thread's words go stale once something that needs the user, or a failure, came after them.
  const newer = [...waiting, ...failed].some((fact) => fact.atMs > (headline?.atMs ?? Number.POSITIVE_INFINITY));
  if (headline && !newer) {
    // A failure from before its words is one the thread has judged: the mark is the thread's
    // (a CI run GitHub calls cancelled that the thread found passed is no failure).
    const tone: LiveTone =
      waiting.length > 0 ? "waiting" : running.length > 0 ? "running" : headline.failed ? "failed" : (latestOf(done)?.tone ?? "done");
    const timer = pick(running);
    return {
      tone,
      headline: headline.text,
      detail: headline.detail ?? undefined,
      sinceMs: tone === "running" ? (timer?.sinceMs ?? headline.atMs) : undefined,
    };
  }
  if (!shown) return null;
  const { kind: _kind, atMs: _atMs, rank: _rank, ...job } = shown;
  return job;
}

/** The board's session's live job; null while there is none or the board is another conversation's. */
export function useLiveJob(conversationId: string): LiveJob | null {
  const json = useBoard((s) => {
    const board = s.board;
    if (!board || board.conversationId !== conversationId) return null;
    const job = liveJob(board);
    return job ? JSON.stringify(job) : null;
  });
  return useMemo(() => (json ? (JSON.parse(json) as LiveJob) : null), [json]);
}
