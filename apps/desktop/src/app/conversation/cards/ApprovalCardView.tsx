import { memo, type ReactNode } from "react";

import { DiffStatView } from "@/app/conversation/cards/common";
import { WaitingRow } from "@/app/conversation/cards/common";
import { WorkerChip } from "@/app/conversation/WorkerChip";
import { ActionCard, ActionCardTitle, ToolBox, DECIDERS, paper } from "@/components/conversation";
import { Badge } from "@/components/controls";
import {
  Check,
  Commit,
  Dock,
  Globe,
  Sparkle,
  X,
} from "@/components/icons";
import type { CardState } from "@/ipc/generated";
import { NEVER_PUSHES_NOTE } from "@/lib/setup";
import { cn } from "@/lib/utils";
import { type ShownApproval, useBoard } from "@/state/board";

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
          {state.similar && ", and similar ones in this conversation"}
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
  approval: ShownApproval,
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
        icon: <Dock />,
        title: `Allow ${request.tool}?`,
        subtitle: byline(by, request.reason),
        body: (
          <>
            {request.escalation && (
              <Badge variant="warning" className="self-start">
                Outside the sandbox
              </Badge>
            )}
            {request.command && <ToolBox>{request.command}</ToolBox>}
            {request.paths.length > 0 && (
              <ToolBox>{request.paths.join("\n")}</ToolBox>
            )}
            {!request.command && request.input && (
              <ToolBox>{request.input}</ToolBox>
            )}
            {request.cwd && (
              <p className="text-muted-foreground px-1 font-mono text-xs">in {request.cwd}</p>
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
            <ToolBox>{subject.argv.map(quote).join(" ")}</ToolBox>
            <p className="text-muted-foreground px-1 font-mono text-xs">in {subject.cwd}</p>
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
        subtitle: "One commit",
        body: <DiffStatView stat={subject.diffStat} />,
      };
    case "action":
      return {
        icon: <Sparkle />,
        title: subject.action,
        subtitle: by,
        body: <ToolBox prose>{subject.details}</ToolBox>,
      };
    case "outline":
      return {
        icon: <Sparkle />,
        title: "Start this plan?",
        subtitle: byline(by, subject.title),
        body: <ToolBox prose>{subject.outline}</ToolBox>,
      };
    case "peerMessage":
      return {
        icon: <Sparkle />,
        title: `Send a message to ${subject.title} in ${subject.project}?`,
        subtitle: by,
        body: <ToolBox prose>{subject.text}</ToolBox>,
      };
    case "browser":
      return {
        icon: <Globe />,
        title: subject.question,
        subtitle: by,
        body: <ToolBox prose>{subject.details}</ToolBox>,
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
    return <WaitingRow icon={shown.icon} quiet>Awaiting approval</WaitingRow>;
  }
  // The rail's approval card as it was answered: what it asked, then who decided.
  return (
    <ActionCard data-card="approval" aria-label="Approval" className={cn(paper, "rounded-thread")}>
      <div className="flex min-w-0 items-start gap-2 px-4 pt-3.5 pb-2.5">
        <span className="text-foreground/65 flex h-5 shrink-0 items-center [&_svg]:size-4.5">{shown.icon}</span>
        <ActionCardTitle detail={shown.subtitle}>{shown.title}</ActionCardTitle>
      </div>
      <div className="flex flex-col gap-2 px-3">{shown.body}</div>
      <div className="fade-in animate-in text-muted-foreground flex items-center justify-end gap-2 px-4 pt-2 pb-4 text-xs duration-300">
        <Resolution state={approval.state} />
      </div>
    </ActionCard>
  );
});
