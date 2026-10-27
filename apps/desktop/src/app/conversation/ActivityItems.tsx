import { type FC } from "react";

import { ActivityGroup } from "@/app/conversation/activity/ActivityGroup";
import { LeadPaused } from "@/app/conversation/activity/groupContext";
import { type Activity, type LeadStep, joinExchanges, thinkingLabel } from "@/app/conversation/activity/group";
import { describeLeadStep, LeadStepRow } from "@/app/conversation/activity/LeadStep";
import { describePeerStep, peerStepRow, usePeerLookup } from "@/app/conversation/activity/PeerRow";
import { went } from "@/app/conversation/activity/peerWords";
import { allowedAt, deniedCall } from "@/app/conversation/activity/words";
import { type SequenceEntry as Entry, sentMessageIds } from "@/app/conversation/blocks";
import type { StatusHead } from "@/app/conversation/liveStatus";
import { entryKey, SequenceEntry } from "@/app/conversation/SequenceEntry";
import { ThinkingLine } from "@/app/conversation/ThinkingLine";
import { StatusLine, type ThreadStatusProps, useThreadStatusHead } from "@/app/conversation/ThreadStatus";
import { useViewConversation } from "@/app/conversation/viewContext";
import { RETRYING, useBoard } from "@/state/board";
import { useRevealing } from "@/state/reveal";
import { KeptScope } from "@/components/conversation";

/**
 * A turn's activity in order (THREAD-UX-PLAN.md §3.1): its work groups and everything else as it
 * is. The live and the folded turn render the same items, so nothing moves when the turn ends.
 * The live turn's newest work has one live line: what waits on the user, else what the lead does
 * ("Thinking", "Waiting for a worker"), else the live thought's heading. When the work ends on a
 * group of steps, that group's row says it, unless its step runs and says itself; otherwise the
 * line goes under the work.
 */
export const ActivityItems: FC<{
  activity: readonly Activity<LeadStep, Entry>[];
  /** The live turn's newest work: what its live line reads from; `null` for any other. */
  live: ThreadStatusProps | null;
  /** Its turn still works: each run of steps is a group's row, one step too (see `ActivityGroup`). */
  working?: boolean;
  /** The live thought, if the lead thinks right now. */
  thought: string | null;
  streaming: (entry: Entry) => boolean;
  /** A streaming reply that is a line between the steps, not an answer. */
  commentary: (entry: Entry) => boolean;
  /** The turn's requests, whose denied approvals its failed calls may be. */
  requestIds: readonly string[];
  /** A follow-up was steered in after this work: its groups sum up at once. */
  summary: boolean;
  /** The header's "{n} denied actions" was clicked: the groups holding them open. */
  revealDenied: boolean;
}> = ({ activity: unjoined, live, working = false, thought, streaming, commentary, requestIds, summary, revealDenied }) => {
  const head = useThreadStatusHead(live);
  // Under a message just steered in, with what ran before it summed up, it thinks: even while that still runs.
  const line: (StatusHead & { thought?: boolean }) | null = !live
    ? null
    : (head ?? (live.quotaWait ? null : thought === null ? (unjoined.length === 0 ? { text: "Thinking", tone: "busy" } : null) : { text: thinkingLabel(thought) || "Thinking", tone: "busy", thought: true }));
  // A message to another session says how its exchange stands itself: the line stays under it.
  const newest = unjoined.at(-1);
  const lastStep = newest?.type === "group" ? newest.items.findLast((item) => item.type === "step") : undefined;
  const inGroup = lastStep?.type === "step" && !["peerSent", "sessionSent", "messageRefused"].includes(lastStep.step.kind.type);
  // A message the running lead just sent, with nothing after it yet: its row still reads as
  // sending and no line goes under it, so the wait that usually follows takes the row over and
  // the turn never shrinks under the user's message.
  const leadRunning = useBoard((s) => {
    const board = s.board;
    return !!live && !live.quotaWait && !!board?.runRequest && live.requestIds.includes(board.runRequest) && (board.run === "running" || board.run === "starting");
  });
  const tasks = useBoard((s) => s.board?.tasks);
  const approvals = useBoard((s) => s.board?.approvals);
  const revealing = useRevealing(useViewConversation()?.id ?? null);
  const peers = usePeerLookup(
    unjoined.flatMap((item) => (item.type === "group" ? item.items.flatMap((step) => (step.type === "step" ? [step.step] : [])) : [])),
  );
  const sentStep = newest?.type === "group" && lastStep !== undefined && newest.items.at(-1) === lastStep
    && lastStep.type === "step" && lastStep.step.kind.type === "peerSent" ? lastStep.step : null;
  const going = leadRunning && thought === null && !live?.thinkingLive && sentStep?.kind.type === "peerSent"
    && sentStep.kind.waits.length === 0 && sentStep.kind.messageIds.length > 0
    && sentStep.kind.messageIds.every((id) => { const record = peers.records[id]; return !!record && went(record.state); })
    ? sentStep : null;
  const activity = unjoined.map((item) => item.type === "group"
    ? { ...item, items: joinExchanges(item.items, going) } : item);
  // What waits on the user, or a retry stalling the turn, still shows.
  const shownLine = going && line?.tone !== "needsYou" && !RETRYING.has(line?.text ?? null) ? null : line;
  // A call the user's Deny stopped reads as declined, not failed.
  const denied = (step: LeadStep) =>
    !!approvals && step.kind.type === "tool" && deniedCall(step.kind, step.atMs, requestIds, Object.values(approvals));
  // A command the user had to allow ran from their answer, not from when it asked.
  const ranFrom = (step: LeadStep): number | undefined =>
    approvals && step.kind.type === "tool" ? allowedAt(step.kind, step.atMs, Object.values(approvals)) : undefined;
  const shown = (step: LeadStep): LeadStep => {
    if (step.kind.type !== "tool") return step;
    const ran = ranFrom(step);
    const timed = ran === undefined ? step : { ...step, atMs: ran };
    return denied(step) ? { ...timed, kind: { ...step.kind, status: "declined" } } : timed;
  };
  return (
    <LeadPaused value={!!live?.quotaWait}>
      {activity.map((item, index) =>
        item.type === "group" ? (
          <KeptScope key={`group:${item.key}`} name={`group:${item.key}`}>
            <ActivityGroup
              items={item.items}
              live={!!live && (!live.quotaWait || !!line) && index === activity.length - 1}
              working={working}
              liveLabel={inGroup && index === activity.length - 1 ? line : null}
              summary={summary}
              describe={(step) => describePeerStep(step, peers, step === going) ?? describeLeadStep(shown(step), tasks)}
              renderStep={(step, key) => <KeptScope key={key} name={`step:${key}`}>{peerStepRow(step, step === going) ?? <LeadStepRow step={shown(step)} />}</KeptScope>}
              reveal={revealing || revealDenied
                ? (step) => (!!revealing && sentMessageIds(step.kind).includes(revealing))
                  || (revealDenied && denied(step))
                : undefined}
            />
          </KeptScope>
        ) : (
          <KeptScope key={entryKey(item.entry)} name={`entry:${entryKey(item.entry)}`}>
            <SequenceEntry entry={item.entry} streaming={streaming(item.entry)} commentary={commentary(item.entry)} />
          </KeptScope>
        ),
      )}
      {!inGroup && shownLine && (shownLine.thought && thought !== null ? <ThinkingLine text={thought} /> : <StatusLine head={shownLine} />)}
    </LeadPaused>
  );
};
