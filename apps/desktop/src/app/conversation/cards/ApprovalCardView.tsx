import {
  Branch,
  Check,
  Commit,
  Globe,
  Sparkle,
  Terminal,
  X,
} from "@openai/apps-sdk-ui/components/Icon";
import { memo, type ReactNode } from "react";

import { DiffStatView } from "@/app/conversation/cards/common";
import { WaitingRow } from "@/app/conversation/cards/common";
import { WorkerChip } from "@/app/conversation/WorkerChip";
import {
  ApprovalCard,
  ApprovalCardCode,
} from "@/components/assistant-ui/elements/approval-card";
import { DECIDERS } from "@/components/transcript/TranscriptRow";
import { Badge } from "@/components/ui/badge";
import type { Approval, CardState } from "@/ipc/generated";
import { NEVER_PUSHES_NOTE } from "@/lib/setup";
import { useBoard } from "@/state/board";

/** Shell-quotes an argument only where needed, so the exact argv reads unambiguously. */
function quote(arg: string): string {
  return /^[\w@%+=:,./-]+$/.test(arg) ? arg : `'${arg.replaceAll("'", `'\\''`)}'`;
}

/** Who decided a card, in words. */
export function Resolution({ state }: { state: CardState }) {
  switch (state.type) {
    case "pending":
      return null;
    case "allowed":
      return (
        <>
          <Check className="text-success size-icon-sm" />
          Allowed by {DECIDERS[state.by]}
          {state.similar && ", and again for this worker"}
        </>
      );
    case "denied":
      return (
        <>
          <X className="text-destructive size-icon-sm" />
          Denied by {DECIDERS[state.by]}
          {state.message && `: ${state.message}`}
        </>
      );
    case "expired":
      return <>Expired: {state.reason}</>;
  }
}

type Shown = { icon: ReactNode; title: ReactNode; subtitle?: ReactNode; body: ReactNode };

/** "[Add tests] · why", leaving out what is missing. */
function byline(by: ReactNode, text: string | null): ReactNode {
  if (!by) return text ?? "";
  return text ? (
    <>
      {by} · {text}
    </>
  ) : (
    by
  );
}

function describe(
  approval: Approval,
  actorId: string | null,
  landingId: string | null,
): Shown {
  // The worker that asks, and the one to land, as their chips.
  const by = actorId === null ? null : <WorkerChip taskId={actorId} />;
  const { subject } = approval;
  switch (subject.type) {
    case "cli": {
      const { request } = subject;
      return {
        icon: <Terminal />,
        title: `Allow ${request.tool}?`,
        subtitle: byline(by, request.reason),
        body: (
          <>
            {request.escalation && (
              <Badge variant="warning" className="self-start">
                Outside the sandbox
              </Badge>
            )}
            {request.command && <ApprovalCardCode>{request.command}</ApprovalCardCode>}
            {request.paths.length > 0 && (
              <ApprovalCardCode>{request.paths.join("\n")}</ApprovalCardCode>
            )}
            {!request.command && request.input && (
              <ApprovalCardCode>{request.input}</ApprovalCardCode>
            )}
            {request.cwd && (
              <p className="text-muted-foreground font-mono text-xs">in {request.cwd}</p>
            )}
          </>
        ),
      };
    }
    case "outwardCommand":
      return {
        icon: <Globe />,
        title: "Run a command that reaches outside?",
        subtitle: byline(by, NEVER_PUSHES_NOTE),
        body: (
          <>
            <ApprovalCardCode>{subject.argv.map(quote).join(" ")}</ApprovalCardCode>
            <p className="text-muted-foreground font-mono text-xs">in {subject.cwd}</p>
          </>
        ),
      };
    case "landing":
      return {
        icon: <Commit />,
        title: (
          <>
            Land {landingId === null ? "this task" : <WorkerChip taskId={landingId} />} on{" "}
            {subject.branch}?
          </>
        ),
        subtitle: "One reviewed commit",
        body: <DiffStatView stat={subject.diffStat} />,
      };
    case "finishSession":
      return {
        icon: <Branch />,
        title: `Merge ${subject.branch} into ${subject.base}?`,
        subtitle: `${subject.commits} commit${subject.commits === 1 ? "" : "s"}`,
        body: <DiffStatView stat={subject.diffStat} />,
      };
    case "action":
      return {
        icon: <Sparkle />,
        title: subject.action,
        subtitle: by,
        body: <p className="text-sm whitespace-pre-wrap">{subject.details}</p>,
      };
  }
}

/** In the thread: a waiting row while the card is in the composer, then who decided. */
export const ApprovalCardView = memo(function ApprovalCardView({ cardId }: { cardId: string }) {
  const approval = useBoard((s) => s.board?.approvals[cardId]);
  const actorId = useBoard((s) =>
    approval?.taskId && s.board?.tasks[approval.taskId] ? approval.taskId : null,
  );
  const landingId = useBoard((s) =>
    approval?.subject.type === "landing" && s.board?.tasks[approval.subject.taskId]
      ? approval.subject.taskId
      : null,
  );
  if (!approval) return null;

  const shown = describe(approval, actorId, landingId);
  if (approval.state.type === "pending") {
    return <WaitingRow icon={shown.icon}>Waiting for your approval</WaitingRow>;
  }
  return (
    <ApprovalCard
      data-card="approval"
      icon={shown.icon}
      title={shown.title}
      subtitle={shown.subtitle}
      pending={false}
      resolution={<Resolution state={approval.state} />}
    >
      {shown.body}
    </ApprovalCard>
  );
});
