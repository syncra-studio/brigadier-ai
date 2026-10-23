import type { FC } from "react";

import { DecidedRow } from "@/app/conversation/activity/DecidedRow";
import { MachineRow } from "@/app/conversation/activity/MachineRow";
import { MergedRow } from "@/app/conversation/activity/MergedRow";
import { Moment } from "@/app/conversation/activity/Moment";
import { QuotaResumed } from "@/app/conversation/activity/QuotaResumed";
import { isTeamStep, TeamStep } from "@/app/conversation/activity/TeamSentence";
import type { BlockOrchestratorStep } from "@/app/conversation/blocks";
import { FeedbackOffer } from "@/app/conversation/FeedbackOffer";

/**
 * The thread's notices among its work (THREAD-UX-PLAN.md §3.1): a judgement call the lead made,
 * the machine pausing the work, a merge the user asked for. Each is one standalone row.
 */

/**
 * A step of the lead that isn't work of its own: a team row, or a notice. Plumbing and what a
 * team sentence already says (a worker created, a report read) show nothing.
 */
export const ThreadStep: FC<{ step: BlockOrchestratorStep }> = ({ step }) => {
  const { kind } = step;
  if (kind.type === "machine") return <MachineRow kind={kind} />;
  if (kind.type === "quotaResumed") return <QuotaResumed provider={kind.provider} atMs={step.atMs} />;
  if (kind.type === "moment") return <Moment text={kind.text} />;
  if (kind.type === "decided") return <DecidedRow kind={kind} />;
  if (isTeamStep(kind)) return <TeamStep kind={kind} atMs={step.atMs} />;
  if (kind.type === "feedbackOffered") return <FeedbackOffer id={kind.id} kind={kind.kind} title={kind.title} body={kind.body} />;
  if (kind.type === "merged") return <MergedRow kind={kind} />;
  return null;
};
