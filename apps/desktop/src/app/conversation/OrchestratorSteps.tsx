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
import { toolWords } from "@/app/conversation/toolWords";
import { machineWords } from "@/app/conversation/rowWords";
import { AgentsPanelContext, useWorkerName, WorkerGlyph, WorkerLine } from "@/app/conversation/WorkerChip";
import { WebSearch } from "@/components/assistant-ui/elements/web-search";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import type { OrchestratorStepKind } from "@/ipc/generated";
import { cn } from "@/lib/utils";
import { useBoard } from "@/state/board";
import { useApp } from "@/state/store";

type Kind = BlockOrchestratorStep["kind"]["type"];

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
export const PLURALS: Record<Kind | "worker", [one: string, many: string]> = {
  tool: ["used a tool", "used tools"],
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
export function summarizeKinds(kinds: readonly (Kind | "worker")[]): string {
  const counts = new Map<Kind | "worker", number>();
  for (const kind of kinds) counts.set(kind, (counts.get(kind) ?? 0) + 1);
  const text = [...counts].map(([kind, count]) => PLURALS[kind][count === 1 ? 0 : 1]).join(", ");
  return text.charAt(0).toUpperCase() + text.slice(1);
}

/** The kind whose icon a run of the thread's work shows: its most frequent. */
function dominant(kinds: readonly (Kind | "worker")[]): Kind | "worker" {
  const counts = new Map<Kind | "worker", number>();
  for (const kind of kinds) counts.set(kind, (counts.get(kind) ?? 0) + 1);
  return [...counts].toSorted((a, b) => b[1] - a[1])[0]?.[0] ?? "created";
}

const RUN_ICONS: Record<Kind | "worker", FC<{ className?: string }>> = { ...ICONS, worker: Sparkle };

/** A grey line of the thread's work. */
export const STEP_ROW = "text-muted-foreground flex min-h-row-sm min-w-0 items-center gap-2 text-sm";
const row = STEP_ROW;

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
      className="text-foreground/90 hover:text-foreground focus-visible:ring-ring/50 rounded-control inline-flex max-w-1/2 min-w-0 shrink-0 items-center gap-1 outline-none focus-visible:ring-1"
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
      return <span className={cn("min-w-0 truncate", kind.status === "inProgress" && "shimmer")}>{toolWords(kind)}</span>;
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
      return <span className="min-w-0 truncate">Searched the web for {kind.query}</span>;
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
    <div data-slot="orchestrator-step" data-kind="machine" className={row}>
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
  const Icon = ICONS[kind.type];
  const line =
    kind.type === "decided" ? (
      <span className="min-w-0 truncate">
        Decided: <WorkerLine text={kind.what} />
      </span>
    ) : (
      label(kind, spec)
    );
  const more = details(kind, spec);
  if (!more) {
    return (
      <div data-slot="orchestrator-step" data-kind={kind.type} className={row}>
        <Icon aria-hidden className="size-icon-md shrink-0" />
        <span className="flex min-w-0 flex-1 items-center gap-1.5">{line}</span>
      </div>
    );
  }
  return (
    <Collapsible data-slot="orchestrator-step" data-kind={kind.type}>
      <CollapsibleTrigger asChild>
        <div
          role="button"
          tabIndex={0}
          onKeyDown={(event) => {
            // A line with a worker's name in it is no button element; keys open it as one.
            if (event.target === event.currentTarget && (event.key === "Enter" || event.key === " ")) {
              event.preventDefault();
              event.currentTarget.click();
            }
          }}
          className={cn(row, "group hover:text-foreground focus-visible:ring-ring/50 rounded-control w-full cursor-pointer text-start outline-none focus-visible:ring-1")}
        >
          <Icon aria-hidden className="size-icon-md shrink-0" />
          <span className="flex min-w-0 items-center gap-1.5">{line}</span>
          <ChevronRight aria-hidden className={OPENER} />
        </div>
      </CollapsibleTrigger>
      <CollapsibleContent className="text-muted-foreground flex max-h-action-list flex-col gap-1 overflow-y-auto ps-6 pt-1 pb-2 text-sm wrap-break-word">
        {more}
      </CollapsibleContent>
    </Collapsible>
  );
};

/** What the orchestrator did between two replies, one grey line per step. */
export const OrchestratorSteps: FC<{ steps: readonly BlockOrchestratorStep[] }> = ({ steps }) => (
  <>
    {steps.map((step) => (
      <StepRow key={step.position} step={step} />
    ))}
  </>
);

/**
 * A run of the thread's work, folded into one line that sums it up ("Created a worker, answered
 * a worker, landed commits") and opens to each of its lines.
 */
export const WorkGroup: FC<{ kinds: readonly (Kind | "worker")[]; children: ReactNode }> = ({ kinds, children }) => {
  const Icon = RUN_ICONS[dominant(kinds)];
  return (
    <Collapsible data-slot="work-group">
      <CollapsibleTrigger className={cn(row, "group hover:text-foreground focus-visible:ring-ring/50 rounded-control w-full text-start outline-none focus-visible:ring-1")}>
        <Icon aria-hidden className="size-icon-md shrink-0" />
        <span className="text-foreground/90 min-w-0 truncate">{summarizeKinds(kinds)}</span>
        <ChevronRight aria-hidden className={OPENER} />
      </CollapsibleTrigger>
      <CollapsibleContent className="flex flex-col">{children}</CollapsibleContent>
    </Collapsible>
  );
};
