import { useMemo } from "react";

import { ciPill, ciWatches, heldMerge, mergeWords, runFailed, shortSha } from "@/app/conversation/activity/slotView";
import { isRunRequest, isWorking } from "@/app/conversation/blocks";
import { questionOpen } from "@/app/conversation/cards/questionRound";
import type { IntegratedCheck, OrchestratorStep, ProviderKind } from "@/ipc/generated";
import { type Board, liveRequest, useBoard } from "@/state/board";

/**
 * The Job card as a live summary of the session right now (J1): one headline, one detail line,
 * and the buttons a state needs. The thread's own headline (`set_headline`) gives the words
 * (J2); before it sets one, or when something that needs the user or failed came after it, the
 * board's events give them, in this order (J5): waiting on you, something failed, something
 * running, done. Running and waiting things show whenever they started; a finished outcome only
 * until the user's next message (J4). It names no worker or Background row: those are the
 * context card's, and the Workers strip's.
 */

/** Which icon the card shows: a spinner, a raised hand, a red cross, a tick, a stop, a merge. */
export type LiveTone = "running" | "waiting" | "failed" | "done" | "stopped" | "merged";

/** A button of the card's: open the CI run, cancel the held merge. */
export type LiveAction = { type: "openCi"; url: string } | { type: "cancelMerge" };

export type LiveJob = {
  tone: LiveTone;
  headline: string;
  detail?: string | undefined;
  /** When what it tells began: a running state's timer counts from it. */
  sinceMs?: number | undefined;
  /** When it ended: an outcome shows how long it took. */
  endedAtMs?: number | undefined;
  actions: LiveAction[];
  /** Only the bare "Working": nothing more specific to say (a run's or plan's card says its own). */
  plain?: true | undefined;
};

/** The board's parts it reads; `headline` is the thread's (`set_headline`), cleared by the user's next message. */
export type LiveInput = Pick<
  Board,
  "tasks" | "approvals" | "questions" | "reviews" | "previews" | "integratedChecks" | "orchestratorSteps" | "requests"
> &
  Partial<Pick<Board, "headline">>;

type Kind = "waiting" | "failed" | "running" | "done";

/** One thing the board says, with when it happened and, for running ones, how much it says. */
type Fact = LiveJob & { kind: Kind; atMs: number; rank: number };

const VENDORS: Record<ProviderKind, string> = { claude: "Claude", codex: "Codex" };

const short = (sha: string) => sha.slice(0, 7);
const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;
const firstLine = (text: string | null | undefined) => text?.split("\n").find((line) => line.trim())?.trim() || undefined;

const checkNumber = (check: IntegratedCheck) => Number(check.id.replace(/^check-/, "")) || 0;

/** The session's own checks, newest last: an overnight run's are its card's. */
function sessionChecks(input: LiveInput): IntegratedCheck[] {
  return Object.values(input.integratedChecks)
    .filter((check) => !check.requests.some(isRunRequest))
    .toSorted((a, b) => checkNumber(a) - checkNumber(b));
}

/** Whether checks with something in them passed on exactly `sha`. */
function passedOn(checks: readonly IntegratedCheck[], sha: string | undefined): boolean {
  if (!sha) return false;
  return checks.some((check) => check.tip === sha && check.state.type === "passed" && check.state.commands.length > 0);
}

function checkFacts(checks: readonly IntegratedCheck[]): Fact[] {
  const latest = checks.at(-1);
  if (!latest) return [];
  const sinceMs = latest.startedAtMs ?? latest.dueAtMs;
  const ended = { sinceMs, endedAtMs: latest.endedAtMs ?? undefined, atMs: latest.endedAtMs ?? sinceMs, actions: [] };
  switch (latest.state.type) {
    case "due":
      return [{ kind: "running", rank: 2, tone: "running", headline: "Checks are next", detail: `They run on ${short(latest.tip)} once the work lands`, sinceMs: latest.dueAtMs, atMs: latest.dueAtMs, actions: [] }];
    case "running":
      return [
        {
          kind: "running",
          rank: 3,
          tone: "running",
          headline: "Running checks",
          detail: latest.state.command ? `Now: ${latest.state.command}` : `On ${short(latest.tip)}`,
          sinceMs,
          atMs: sinceMs,
          actions: [],
        },
      ];
    case "passed": {
      const count = latest.state.commands.length;
      if (count === 0) return [];
      return [{ ...ended, kind: "done", rank: 0, tone: "done", headline: `Checks passed on ${short(latest.tip)}`, detail: `${count} of ${count} passed` }];
    }
    case "failed":
      return [{ ...ended, kind: "failed", rank: 0, tone: "failed", headline: "Checks failed", detail: `${latest.state.command}: ${latest.state.status}` }];
    case "interrupted":
      return [{ ...ended, kind: "failed", rank: 0, tone: "failed", headline: "Checks couldn’t finish", detail: latest.state.reason }];
  }
}

function reviewFacts(input: LiveInput): Fact[] {
  const reviews = Object.values(input.reviews)
    .filter((review) => !isRunRequest(review.requestId))
    .toSorted((a, b) => a.startedAtMs - b.startedAtMs);
  const running = reviews.filter((review) => review.state.type === "running");
  if (running.length > 0) {
    const first = running[0]!;
    const who = [...new Set(running.map((review) => VENDORS[review.reviewer]))].join(" and ");
    return [
      {
        kind: "running",
        rank: 4,
        tone: "running",
        headline: `${who} ${running.length > 1 && who.includes(" and ") ? "are" : "is"} reviewing`,
        detail: running.length > 1 ? `${running.length} reviews at work` : `The changes up to ${short(first.tip)}`,
        sinceMs: first.startedAtMs,
        atMs: first.startedAtMs,
        actions: [],
      },
    ];
  }
  const latest = reviews.at(-1);
  if (!latest || latest.endedAtMs === null) return [];
  const ended = { sinceMs: latest.startedAtMs, endedAtMs: latest.endedAtMs, atMs: latest.endedAtMs, actions: [] };
  const from = `From ${VENDORS[latest.reviewer]}’s review of ${short(latest.tip)}`;
  switch (latest.state.type) {
    case "clean":
      return [{ ...ended, kind: "done", rank: 0, tone: "done", headline: "Review found nothing to fix", detail: from }];
    case "findings": {
      const count = latest.items.length > 0 ? latest.items.length : latest.state.count;
      return [{ ...ended, kind: "done", rank: 0, tone: "done", headline: `Review found ${plural(count, "issue")}`, detail: from }];
    }
    case "failed":
      return [{ ...ended, kind: "failed", rank: 0, tone: "failed", headline: "The review couldn’t run", detail: firstLine(latest.state.reason) }];
    default:
      return [];
  }
}

function ciFacts(steps: readonly OrchestratorStep[]): Fact[] {
  const all = ciWatches(steps.filter((step) => !isRunRequest(step.requestId)));
  const live = all.filter((watch) => watch.result === null);
  // While a push is checked, its watch is the card's; once none is, the newest one's outcome.
  const watches = live.length > 0 ? live : all.slice(-1);
  const newest = watches.at(-1);
  if (!newest) return [];
  const pill = ciPill(watches);
  const runs = watches.flatMap((watch) => watch.runs);
  const failedRun = runs.find(runFailed);
  const openRun = failedRun ?? runs.find((run) => run.status !== "completed") ?? runs[0];
  const actions: LiveAction[] = openRun?.url ? [{ type: "openCi", url: openRun.url }] : [];
  const detail = `${newest.branch} · ${shortSha(newest.sha)}`;
  const sinceMs = watches[0]!.sinceMs;
  const endedAtMs = live.length > 0 ? undefined : (newest.endedAtMs ?? undefined);
  const atMs = endedAtMs ?? sinceMs;
  if (pill.state === "failed") return [{ kind: "failed", rank: 0, tone: "failed", headline: pill.words, detail, sinceMs, endedAtMs, atMs, actions }];
  if (pill.state === "checking" || live.length > 0) return [{ kind: "running", rank: 5, tone: "running", headline: pill.words, detail, sinceMs, atMs, actions }];
  if (pill.state === "passed") return [{ kind: "done", rank: 0, tone: "done", headline: pill.words, detail, sinceMs, endedAtMs, atMs, actions: [] }];
  return [];
}

function mergeFacts(input: LiveInput, checks: readonly IntegratedCheck[]): Fact[] {
  const facts: Fact[] = [];
  const steps = input.orchestratorSteps.filter((step) => !isRunRequest(step.requestId));
  const held = heldMerge(steps, input.tasks, Object.values(input.questions));
  // A held merge whose card asks: the question below is what waits, its own fact.
  if (held && !held.asking) {
    facts.push({
      kind: "running",
      rank: 6,
      tone: "running",
      headline: mergeWords(held),
      detail: `Into ${held.base} once they’re done, as you asked`,
      sinceMs: held.sinceMs,
      atMs: held.sinceMs,
      actions: [{ type: "cancelMerge" }],
    });
  }
  const last = steps.findLast((step) => ["mergeRequested", "merged", "mergeCancelled", "mergeStopped"].includes(step.kind.type));
  if (last?.kind.type === "mergeRequested") {
    const latest = checks.at(-1);
    facts.push({
      kind: "running",
      rank: 7,
      tone: "running",
      headline: `Merging into ${last.kind.base}`,
      detail: latest && passedOn([latest], latest.tip) ? `Checks passed on ${short(latest.tip)}` : `${last.kind.branch} into ${last.kind.base}`,
      sinceMs: last.atMs,
      atMs: last.atMs,
      actions: [],
    });
  }
  if (last?.kind.type === "merged") {
    const kind = last.kind;
    const head = kind.head ?? kind.record?.head;
    const sha = kind.mergeCommit ?? head;
    const detail = [sha && short(sha), kind.pr !== undefined && `#${kind.pr}`, plural(kind.commits, "commit")].filter(Boolean).join(" · ");
    facts.push({
      kind: "done",
      rank: 0,
      tone: "merged",
      // Only checks that passed on the very commit that merged count: never claim a pass.
      headline: `Merged into ${kind.base}${passedOn(checks, head) ? " · checks passed" : ""}`,
      detail,
      atMs: last.atMs,
      actions: [],
    });
  }
  return facts;
}

/** When the newest of `items` came. */
const newest = (items: readonly { createdAtMs: number }[]) => Math.max(...items.map((item) => item.createdAtMs));

function waitingFacts(input: LiveInput): Fact[] {
  const facts: Fact[] = [];
  const open = Object.values(input.questions).filter(questionOpen);
  const merges = open.filter((question) => question.kind.type === "merge");
  const others = open.filter((question) => question.kind.type !== "merge");
  if (merges.length > 0) {
    facts.push({ kind: "waiting", rank: 0, tone: "waiting", headline: "Waiting for your yes below", detail: "To merge the work", atMs: newest(merges), actions: [] });
  }
  if (others.length > 0) {
    facts.push({
      kind: "waiting",
      rank: 0,
      tone: "waiting",
      headline: others.length === 1 ? "Waiting for your answer below" : `Waiting for ${others.length} answers below`,
      atMs: newest(others),
      actions: [],
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
      actions: [],
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
      actions: [],
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
      actions: [],
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
  return [{ kind: "done", rank: 0, tone: "stopped", headline: "Stopped", detail: "You stopped it", sinceMs: last.startedAtMs, endedAtMs, atMs: endedAtMs, actions: [] }];
}

/** When the user last wrote: finished outcomes from before it are gone (J4). */
function lastMessageAtMs(input: LiveInput): number {
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
  const checks = sessionChecks(input);
  const facts = [...waitingFacts(input), ...workFailures(input), ...checkFacts(checks), ...reviewFacts(input), ...ciFacts(input.orchestratorSteps), ...mergeFacts(input, checks), ...stopFacts(input)];
  // The thread or a worker at work, with nothing more to say: Working, naming no worker (J3).
  const live = liveRequest(input);
  const workers = Object.values(input.tasks).filter((task) => isWorking(task) && !isRunRequest(task.requestId));
  const startedAtMs = live ? (live.live.worked?.at(-1)?.fromMs ?? live.live.startedAtMs) : Math.min(...workers.map((task) => task.createdAtMs));
  if (live || workers.length > 0) {
    facts.push({ kind: "running", rank: 1, tone: "running", headline: "Working", sinceMs: startedAtMs, atMs: startedAtMs, actions: [], plain: true });
  }
  const waiting = facts.filter((fact) => fact.kind === "waiting");
  // A failure or an outcome from before the user's last message is over with.
  const failed = facts.filter((fact) => fact.kind === "failed" && fact.atMs >= since);
  const running = facts.filter((fact) => fact.kind === "running");
  const done = facts.filter((fact) => fact.kind === "done" && fact.atMs >= since);
  // Every button of what is still going stays reachable, whatever the headline says.
  const actions = [...waiting, ...failed, ...running]
    .flatMap((fact) => fact.actions)
    .filter((action, index, all) => all.findIndex((other) => other.type === action.type) === index);

  // The bare Working says least: an outcome since the user's last message says more.
  const specific = running.filter((fact) => !fact.plain);
  const shown = pick(waiting) ?? latestOf(failed) ?? pick(specific) ?? latestOf(done) ?? pick(running);
  const headline = input.headline && input.headline.atMs >= since ? input.headline : null;
  // The thread's words go stale once something that needs the user, or a failure, came after them.
  const newer = [...waiting, ...failed].some((fact) => fact.atMs > (headline?.atMs ?? Number.POSITIVE_INFINITY));
  if (headline && !newer) {
    const tone: LiveTone = waiting.length > 0 ? "waiting" : running.length > 0 ? "running" : failed.length > 0 ? "failed" : (shown?.tone ?? "done");
    const timer = pick(running);
    return {
      tone,
      headline: headline.text,
      detail: headline.detail ?? undefined,
      sinceMs: tone === "running" ? (timer?.sinceMs ?? headline.atMs) : undefined,
      actions,
    };
  }
  if (!shown) return null;
  const { kind: _kind, atMs: _atMs, rank: _rank, ...job } = shown;
  return { ...job, actions };
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
