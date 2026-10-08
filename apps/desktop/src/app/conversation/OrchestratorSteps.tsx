import {
  Book,
  Branch,
  Chat,
  CheckCircle,
  Clock,
  Commit,
  Globe,
  PauseCircle,
  PlayCircle,
  Sparkle,
} from "@openai/apps-sdk-ui/components/Icon";
import { type FC, type ReactNode, useContext } from "react";

import type { BlockOrchestratorStep, DecidedStep, MachineWords } from "@/app/conversation/blocks";
import { machineWords } from "@/app/conversation/rowWords";
import { AgentsPanelContext, useWorkerName, WorkerGlyph, WorkerLine } from "@/app/conversation/WorkerChip";
import { ThreadActivity } from "@/components/assistant-ui/elements/thread-activity";
import { ROW, ROW_DETAIL } from "@/components/assistant-ui/elements/activity-row";
import type { OrchestratorStepKind } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { useBoard } from "@/state/board";
import { useApp } from "@/state/store";

type Kind = BlockOrchestratorStep["kind"]["type"];

/**
 * The lead's steps about its team and the notices among its work (decisions, the machine): one
 * row each. Its own work (tools, reads, searches) is in `activity/`.
 */
const ICONS: Record<Kind, FC<{ className?: string }>> = {
  tool: Book,
  messaged: Chat,
  readReport: Book,
  readArtifact: Book,
  accepted: Commit,
  searchedWeb: Globe,
  readPage: Globe,
  decided: CheckCircle,
  created: Sparkle,
  answered: Chat,
  landed: Commit,
  merged: Branch,
  machine: Clock,
};

function hostOf(url: string): string {
  try {
    return new URL(url).host || url;
  } catch {
    return url;
  }
}

/**
 * A worker named in a line, as plain words that open its thread: "Lead · Phase 1".
 * `attached` words follow it closely, as a possessive does.
 */
export const WorkerName: FC<{ taskId: string }> = ({ taskId }) => {
  const { setPanel } = useContext(AgentsPanelContext);
  const name = useWorkerName(taskId);
  if (name === null) return <span className="shrink-0">a worker</span>;
  return (
    <button
      type="button"
      data-slot="worker-name"
      onClick={(event) => {
        event.stopPropagation();
        setPanel(taskId);
      }}
      title={`Open ${name}`}
      className="text-foreground/90 hover:text-foreground rounded-control inline-flex max-w-1/2 min-w-0 shrink-0 items-center gap-1"
    >
      <WorkerGlyph taskId={taskId} className="size-icon-xs shrink-0" />
      <span className="min-w-0 truncate">{name}</span>
    </button>
  );
};

/** Plain words of a line that names workers: they keep their width, the names give way. */
const Words: FC<{ attached?: boolean; children: ReactNode }> = ({ attached, children }) => (
  <span className={cn("shrink-0 whitespace-nowrap", attached && "-ms-1.5")}>{children}</span>
);

/** The first line of a worker's instructions, for its "Created …" line. */
function firstLine(text: string): string {
  return (text.split("\n").find((line) => line.trim()) ?? "").replace(/^#+\s*/, "").trim();
}

/** A step's line; a worker it names opens its thread. */
function label(kind: OrchestratorStepKind, spec: string | null): ReactNode {
  switch (kind.type) {
    case "tool":
      return <span className="min-w-0 truncate">{kind.name}</span>;
    case "messaged":
      return (
        <>
          <Words>Sent message to</Words>
          <WorkerName taskId={kind.taskId} />
        </>
      );
    case "readReport":
      return (
        <>
          <Words>Read</Words>
          <WorkerName taskId={kind.taskId} />
          <Words attached>’s report</Words>
        </>
      );
    case "readArtifact":
      return <span className="min-w-0 truncate">Read {kind.name}</span>;
    case "accepted":
      return (
        <>
          <Words>Accepted</Words>
          <WorkerName taskId={kind.taskId} />
          <Words attached>’s change</Words>
        </>
      );
    case "searchedWeb":
      return <span className="min-w-0 truncate">Searched the web</span>;
    case "readPage":
      return <span className="min-w-0 truncate">Read {hostOf(kind.url)}</span>;
    case "created":
      return (
        <>
          <Words>Created</Words>
          <WorkerName taskId={kind.taskId} />
          {spec && <span className="min-w-0 truncate">with the instructions: {firstLine(spec)}</span>}
        </>
      );
    case "answered":
      return (
        <>
          <Words>Answered</Words>
          <WorkerName taskId={kind.taskId} />
          <span className="-ms-1.5 min-w-0 truncate">
            {": "}
            {kind.answer}
            {kind.why && ` — ${kind.why}`}
          </span>
        </>
      );
    case "landed":
      return (
        <span className="min-w-0 truncate">
          Landed {kind.commits} {kind.commits === 1 ? "commit" : "commits"} on {kind.branch}
        </span>
      );
    case "merged":
      return (
        <span className="min-w-0 truncate">
          Merged {kind.branch} into {kind.base}
        </span>
      );
  }
}

/** What a step opens to, when it says more than its line: the instructions, the question and answer. */
function details(kind: OrchestratorStepKind | DecidedStep, spec: string | null): ReactNode {
  switch (kind.type) {
    case "decided":
      return kind.why ? (
        <>
          <span className="text-foreground/80"><WorkerLine text={kind.what} /></span>
          <span><WorkerLine text={kind.why} /></span>
        </>
      ) : null;
    case "created":
      return spec ? <p className="text-foreground/80 whitespace-pre-wrap">{spec}</p> : null;
    case "answered":
      return (
        <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1">
          {kind.question && (
            <>
              <dt>Asked</dt>
              <dd className="text-foreground/80 whitespace-pre-wrap">{kind.question}</dd>
            </>
          )}
          <dt>Answer</dt>
          <dd className="text-foreground/80 whitespace-pre-wrap">{kind.answer}</dd>
          {kind.why && (
            <>
              <dt>Why</dt>
              <dd className="whitespace-pre-wrap">{kind.why}</dd>
            </>
          )}
        </dl>
      );
    default:
      return null;
  }
}

const MACHINE_ICONS: Record<MachineWords["machine"], FC<{ className?: string }>> = {
  waitingToCool: Clock,
  waitingForBuild: Clock,
  paused: PauseCircle,
  resumed: PlayCircle,
};

/** A grey row about the machine; it names the worker when it is about one. */
const MachineRow: FC<{ kind: MachineWords }> = ({ kind }) => {
  const mac = useApp((s) => s.info?.platform === "macos");
  const Icon = MACHINE_ICONS[kind.machine];
  return (
    <div data-slot="orchestrator-step" data-kind="machine" className={ROW}>
      <Icon aria-hidden className="size-4 shrink-0" />
      <span className="flex min-w-0 flex-1 items-center gap-1.5">
        {kind.taskId && <WorkerName taskId={kind.taskId} />}
        <span className="min-w-0 truncate">{machineWords(kind.machine, kind.command, mac ? "the Mac" : "the computer")}</span>
      </span>
    </div>
  );
};

/**
 * One step of the orchestrator as a grey line ("Created Lead · Phase 1 with the instructions:
 * …", "Answered Lead · Phase 1: … — why"); one that says more opens to it.
 */
export const StepRow: FC<{ step: BlockOrchestratorStep }> = ({ step }) =>
  step.kind.type === "machine" ? <MachineRow kind={step.kind} /> : <WorkStepRow kind={step.kind} />;

const WorkStepRow: FC<{ kind: OrchestratorStepKind | DecidedStep }> = ({ kind }) => {
  const spec = useBoard((s) => (kind.type === "created" ? (s.board?.tasks[kind.taskId]?.spec ?? null) : null));
  const Icon = ICONS[kind.type];
  const line =
    kind.type === "decided" ? (
      <span className="min-w-0 truncate">
        Decided: <WorkerLine text={kind.what} />
      </span>
    ) : (
      label(kind, spec)
    );
  return <ThreadActivity data-slot="orchestrator-step" data-kind={kind.type} className={ROW}
    detail={details(kind, spec)} detailClassName={cn(ROW_DETAIL, "wrap-break-word")}>
    <Icon aria-hidden className="size-4 shrink-0" />
    <span className="flex min-w-0 flex-1 items-center gap-1.5">{line}</span>
  </ThreadActivity>;
};
