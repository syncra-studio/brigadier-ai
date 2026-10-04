import {
  Book,
  Chat,
  Check,
  CheckCircle,
  ChevronRight,
  Globe,
} from "@openai/apps-sdk-ui/components/Icon";
import type { FC, ReactNode } from "react";

import type { BlockOrchestratorStep, DecidedStep } from "@/app/conversation/blocks";
import { plainLine } from "@/app/conversation/rowWords";
import { WorkerMention } from "@/app/conversation/WorkerChip";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import type { OrchestratorStepKind } from "@/ipc/generated";
import { cn } from "@/lib/utils";

type Kind = BlockOrchestratorStep["kind"]["type"];

const ICONS: Record<Kind, FC<{ className?: string }>> = {
  messaged: Chat,
  readReport: Book,
  readArtifact: Book,
  accepted: Check,
  searchedWeb: Globe,
  readPage: Globe,
  decided: CheckCircle,
};

/** How a run of steps sums them up ("Read reports, messaged a worker"). */
const PLURALS: Record<Kind, [one: string, many: string]> = {
  messaged: ["messaged a worker", "messaged workers"],
  readReport: ["read a report", "read reports"],
  readArtifact: ["read a file", "read files"],
  accepted: ["accepted a change", "accepted changes"],
  searchedWeb: ["searched the web", "searched the web"],
  readPage: ["read a page", "read pages"],
  decided: ["decided for you", "decided for you"],
};

/** A grey line of the thread's work. */
export const STEP_ROW = "text-muted-foreground flex min-h-row-sm min-w-0 items-center gap-2 text-sm";
const row = STEP_ROW;

function hostOf(url: string): string {
  try {
    return new URL(url).host || url;
  } catch {
    return url;
  }
}

/**
 * Plain words of a line that names workers: they keep their width, the chips give way.
 * `attached` words follow a chip closely, as a possessive does.
 */
const Words: FC<{ attached?: boolean; children: ReactNode }> = ({ attached, children }) => (
  <span className={cn("shrink-0 whitespace-nowrap", attached && "-ms-1")}>{children}</span>
);

/** A step's line; a worker it names is its chip ("Accepted [Add tests]’s change"). */
function label(kind: OrchestratorStepKind | DecidedStep): ReactNode {
  switch (kind.type) {
    case "decided":
      return <span className="min-w-0 truncate">Decided for you: {plainLine(kind.what)}</span>;
    case "messaged":
      return (
        <>
          <Words>Sent message to</Words>
          <WorkerMention taskId={kind.taskId} />
        </>
      );
    case "readReport":
      return (
        <>
          <Words>Read</Words>
          <WorkerMention taskId={kind.taskId} />
          <Words attached>’s report</Words>
        </>
      );
    case "readArtifact":
      return <span className="min-w-0 truncate">Read {kind.name}</span>;
    case "accepted":
      return (
        <>
          <Words>Accepted</Words>
          <WorkerMention taskId={kind.taskId} />
          <Words attached>’s change</Words>
        </>
      );
    case "searchedWeb":
      return <span className="min-w-0 truncate">Searched the web for {kind.query}</span>;
    case "readPage":
      return <span className="min-w-0 truncate">Read {hostOf(kind.url)}</span>;
  }
}

/** A judgement call made for the user: its line, which opens to why. */
const DecidedRow: FC<{ what: string; why: string }> = ({ what, why }) => {
  const line = (
    <>
      <CheckCircle aria-hidden className="size-icon-md shrink-0" />
      <span className="min-w-0 truncate">Decided for you: {plainLine(what)}</span>
    </>
  );
  if (!why) {
    return (
      <div data-slot="orchestrator-step" data-kind="decided" className={row}>
        {line}
      </div>
    );
  }
  return (
    <Collapsible data-slot="orchestrator-step" data-kind="decided">
      <CollapsibleTrigger className={cn(row, "group hover:text-foreground w-full text-start")}>
        {line}
        <ChevronRight
          aria-hidden
          className="size-icon-xs shrink-0 opacity-0 transition-[rotate,opacity] group-hover:opacity-100 group-data-[state=open]:rotate-90 group-data-[state=open]:opacity-100"
        />
      </CollapsibleTrigger>
      <CollapsibleContent className="text-muted-foreground flex flex-col gap-1 ps-6 pb-1 text-sm wrap-break-word">
        <span className="text-foreground/80">{plainLine(what)}</span>
        <span>{plainLine(why)}</span>
      </CollapsibleContent>
    </Collapsible>
  );
};

const StepRow: FC<{ step: BlockOrchestratorStep }> = ({ step }) => {
  if (step.kind.type === "decided") return <DecidedRow what={step.kind.what} why={step.kind.why} />;
  const Icon = ICONS[step.kind.type];
  return (
    <div
      data-slot="orchestrator-step"
      data-kind={step.kind.type}
      className={row}
    >
      <Icon aria-hidden className="size-icon-md shrink-0" />
      <span className="flex min-w-0 flex-1 items-center gap-1.5">{label(step.kind)}</span>
    </div>
  );
};

/**
 * What the orchestrator did between two replies: one grey line per step ("Sent message to
 * Add tests"), and a run of steps summed up in one line that opens to each of them.
 */
export const OrchestratorSteps: FC<{ steps: readonly BlockOrchestratorStep[] }> = ({ steps }) => {
  const [first] = steps;
  if (steps.length === 1 && first) return <StepRow step={first} />;
  const counts = new Map<Kind, number>();
  for (const step of steps) counts.set(step.kind.type, (counts.get(step.kind.type) ?? 0) + 1);
  const [dominant] = [...counts].toSorted((a, b) => b[1] - a[1])[0] ?? ["messaged" as const];
  const Icon = ICONS[dominant];
  const summary = [...counts].map(([kind, count]) => PLURALS[kind][count === 1 ? 0 : 1]).join(", ");
  return (
    <Collapsible data-slot="orchestrator-steps">
      <CollapsibleTrigger className={cn(row, "group hover:text-foreground w-full text-start")}>
        <Icon aria-hidden className="size-icon-md shrink-0" />
        <span className="min-w-0 truncate">
          {summary.charAt(0).toUpperCase() + summary.slice(1)}
        </span>
        <ChevronRight
          aria-hidden
          className="size-icon-xs shrink-0 opacity-0 transition-[rotate,opacity] group-hover:opacity-100 group-data-[state=open]:rotate-90 group-data-[state=open]:opacity-100"
        />
      </CollapsibleTrigger>
      <CollapsibleContent className="max-h-action-list overflow-y-auto ps-6">
        {steps.map((step) => (
          <StepRow key={step.position} step={step} />
        ))}
      </CollapsibleContent>
    </Collapsible>
  );
};
