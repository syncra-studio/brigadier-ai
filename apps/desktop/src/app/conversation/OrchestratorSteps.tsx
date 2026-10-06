import {
  Book,
  Branch,
  Chat,
  CheckCircle,
  ChevronRight,
  Clock,
  Commit,
  Globe,
  PauseCircle,
  PlayCircle,
  Sparkle,
} from "@openai/apps-sdk-ui/components/Icon";
import { type FC, type ReactNode, useContext } from "react";

import type { BlockOrchestratorStep, DecidedStep, MachineWords } from "@/app/conversation/blocks";
import { toolActivity, toolName, toolWords, type ToolKind } from "@/app/conversation/toolWords";
import { machineWords } from "@/app/conversation/rowWords";
import { AgentsPanelContext, useWorkerName, WorkerGlyph, WorkerLine } from "@/app/conversation/WorkerChip";
import { ThreadActivity } from "@/components/assistant-ui/elements/thread-activity";
import { ACTIVITY_ROW, ACTIVITY_DETAIL, ACTIVITY_ICONS } from "@/components/assistant-ui/elements/activity-row";
import { WebSearch } from "@/components/assistant-ui/elements/web-search";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import type { OrchestratorStepKind } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { useBoard } from "@/state/board";
import { useApp } from "@/state/store";

type Kind = BlockOrchestratorStep["kind"]["type"];
export type SummaryKind = Kind | ToolKind | "worker";

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

/** How a run of the thread's work sums it up ("Created a worker, answered a worker"). */
export const PLURALS: Record<SummaryKind, [one: string, many: string]> = {
  tool: ["completed an action", "completed actions"],
  read: ["read a file", "read files"],
  search: ["searched files", "searched files"],
  list: ["listed files", "listed files"],
  edit: ["edited a file", "edited files"],
  run: ["ran a command", "ran commands"],
  message: ["sent a message", "sent messages"],
  memory: ["saved project memory", "saved project memory"],
  plan: ["planned the work", "planned the work"],
  approval: ["requested approval", "requested approvals"],
  land: ["landed changes", "landed changes"],
  report: ["sent a report", "sent reports"],
  web: ["searched the web", "searched the web"],
  image: ["created an image", "created images"],
  messaged: ["messaged a worker", "messaged workers"],
  readReport: ["read a report", "read reports"],
  readArtifact: ["read a file", "read files"],
  accepted: ["accepted a change", "accepted changes"],
  searchedWeb: ["searched the web", "searched the web"],
  readPage: ["read a page", "read pages"],
  decided: ["made a decision", "made decisions"],
  created: ["created a worker", "created workers"],
  answered: ["answered a worker", "answered workers"],
  landed: ["landed commits", "landed commits"],
  merged: ["merged a branch", "merged branches"],
  machine: ["waited for the computer", "waited for the computer"],
  worker: ["ran a worker", "ran workers"],
};

/** "Created workers, answered a worker": the kinds of a run, in the order they first came. */
export function summarizeKinds(kinds: readonly SummaryKind[], actions: readonly (string | null)[] = []): string {
  const counts = new Map<string, { kind: SummaryKind; action: string | null; count: number }>();
  kinds.forEach((kind, index) => {
    const action = actions[index] ?? null;
    const key = action ?? kind;
    const known = counts.get(key);
    counts.set(key, { kind, action, count: (known?.count ?? 0) + 1 });
  });
  const text = [...counts.values()].map(({ kind, action, count }) => action?.toLowerCase() ?? PLURALS[kind][count === 1 ? 0 : 1]).join(", ");
  return text.charAt(0).toUpperCase() + text.slice(1);
}

/** The kind whose icon a run of the thread's work shows: its most frequent. */
function dominant(kinds: readonly SummaryKind[]): SummaryKind {
  const counts = new Map<SummaryKind, number>();
  for (const kind of kinds) counts.set(kind, (counts.get(kind) ?? 0) + 1);
  return [...counts].toSorted((a, b) => b[1] - a[1])[0]?.[0] ?? "created";
}

const RUN_ICONS: Record<SummaryKind, FC<{ className?: string }>> = { ...ACTIVITY_ICONS, ...ICONS, worker: Sparkle };

/** A grey line of the thread's work. */
export const STEP_ROW = "text-muted-foreground flex min-h-row-sm min-w-0 items-center gap-2 text-sm";

/** The chevron at the end of a line that opens: shown on hover, turned while open. */
export const OPENER =
  "size-icon-xs shrink-0 opacity-0 transition-[rotate,opacity] group-hover:opacity-100 group-focus-visible:opacity-100 group-data-[state=open]:rotate-90 group-data-[state=open]:opacity-100";

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
function label(kind: OrchestratorStepKind, spec: string | null, toolLabel: string): ReactNode {
  switch (kind.type) {
    case "tool":
      return <span title={toolLabel} className={cn("min-w-0 truncate", kind.status === "inProgress" && "shimmer")}>{toolLabel}</span>;
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
    case "searchedWeb":
      return <WebSearch query={kind.query} results={[]} />;
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
    <div data-slot="orchestrator-step" data-kind="machine" className={STEP_ROW}>
      <Icon aria-hidden className="size-icon-md shrink-0" />
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
  const toolLabel = useBoard((s) => kind.type === "tool" ? toolWords(kind, s.board?.tasks) : "");
  const Icon = kind.type === "tool" ? ACTIVITY_ICONS[toolActivity(kind.name).kind] : ICONS[kind.type];
  const line =
    kind.type === "decided" ? (
      <span className="min-w-0 truncate">
        Decided: <WorkerLine text={kind.what} />
      </span>
    ) : (
      label(kind, spec, toolLabel)
    );
  const workerRow = isWorkerKind(kind);
  const row = workerRow ? STEP_ROW : ACTIVITY_ROW;
  const more = details(kind, spec);
  return <ThreadActivity data-slot="orchestrator-step" data-kind={kind.type} className={row}
    detail={more} detailClassName={workerRow ? "text-muted-foreground flex max-h-action-list flex-col gap-1 overflow-y-auto ps-6 pt-1 pb-2 text-sm wrap-break-word" : cn(ACTIVITY_DETAIL, "wrap-break-word")}>
    <Icon aria-hidden className="size-icon-md shrink-0" />
    <span className="flex min-w-0 flex-1 items-center gap-1.5">{line}</span>
  </ThreadActivity>;
};

/** What the orchestrator did between two replies, one grey line per step. */
export const OrchestratorSteps: FC<{ steps: readonly BlockOrchestratorStep[]; grouped?: boolean }> = ({ steps, grouped = true }) => {
  const runs: BlockOrchestratorStep[][] = [];
  for (const step of steps) {
    const last = runs.at(-1);
    const foldable = isNonWorkerStep(step) && (step.kind.type !== "tool" || step.kind.status !== "inProgress");
    if (foldable && last?.every((item) => isNonWorkerStep(item) && (item.kind.type !== "tool" || item.kind.status !== "inProgress"))) last.push(step);
    else runs.push([step]);
  }
  return <>
    {runs.map((run) => grouped && run.length > 1
      ? <WorkGroup key={run[0]!.position} kinds={run.map(stepSummaryKind)} actions={run.map(stepSummaryAction)}>{run.map((step) => <StepRow key={step.position} step={step} />)}</WorkGroup>
      : run.map((step) => <StepRow key={step.position} step={step} />))}
  </>;
};

function isWorkerKind(kind: BlockOrchestratorStep["kind"]): boolean {
  if (kind.type === "tool") return ["worker", "message", "report"].includes(toolActivity(kind.name).kind) || toolName(kind.name) === "read_report";
  return ["created", "messaged", "readReport", "accepted", "answered", "machine"].includes(kind.type);
}

export function isNonWorkerStep(step: BlockOrchestratorStep): boolean {
  return !isWorkerKind(step.kind);
}

export function stepSummaryAction(step: BlockOrchestratorStep): string | null {
  if (step.kind.type !== "tool") return null;
  const activity = toolActivity(step.kind.name);
  // Standard file/command verbs pluralize; memory and project searches keep their meaning.
  return ["search", "list", "memory", "plan", "tool"].includes(activity.kind) ? activity.done : null;
}

export function stepSummaryKind(step: BlockOrchestratorStep): SummaryKind {
  return step.kind.type === "tool" ? toolActivity(step.kind.name).kind : step.kind.type;
}

/**
 * A run of the thread's work, folded into one line that sums it up ("Created a worker, answered
 * a worker, landed commits") and opens to each of its lines.
 */
export const WorkGroup: FC<{ kinds: readonly SummaryKind[]; actions?: readonly (string | null)[]; children: ReactNode }> = ({ kinds, actions, children }) => {
  const Icon = RUN_ICONS[dominant(kinds)];
  const workerGroup = kinds.some((kind) => ["worker", "created", "messaged", "readReport", "accepted", "answered", "message", "report", "machine"].includes(kind));
  const row = workerGroup ? STEP_ROW : ACTIVITY_ROW;
  return (
    <Collapsible data-slot="work-group">
      <CollapsibleTrigger className={cn(row, "group hover:text-foreground rounded-control w-full text-start")}>
        <Icon aria-hidden className="size-icon-md shrink-0" />
        <span className={cn("min-w-0 truncate", workerGroup && "text-foreground/90")}>{summarizeKinds(kinds, actions)}</span>
        <ChevronRight aria-hidden className={OPENER} />
      </CollapsibleTrigger>
      <CollapsibleContent className={workerGroup ? "flex flex-col" : "flex min-w-0 flex-col gap-1 pt-1"}>{children}</CollapsibleContent>
    </Collapsible>
  );
};
